//! --- Application-wide state entity. ---
//!
//! --- `AppState` is the root entity that holds all shared application data. ---
//! It is created once at startup and passed to all UI components via GPUI's
//! --- entity subscription/observation mechanism. ---

use crate::core::db::Database;
use crate::core::filters::ClipboardFilters;
use crate::core::html_text;
use crate::core::i18n_keys::I18nKey;
use crate::core::settings::AppSettings;
use crate::core::transfer_types::{
    TRANSFER_BLUE, TRANSFER_STATUS_CLOUD_UID, TRANSFER_STATUS_DOWNLOADING_UID,
    TRANSFER_STATUS_LOCAL_UID, TRANSFER_STATUS_PINNED_UID, TRANSFER_STATUS_RETENTION_UID,
};
use crate::core::types::next_tag_color;
use crate::core::types::ClipboardItem;
use crate::core::types::ContentType;
use crate::core::types::DisplayKind;
use crate::core::types::FileData;
use crate::core::types::RichData;
use crate::core::types::TagInfo;
use crate::services::update::{UpdateInfo, UpdatePhase};
use crate::state::sync::SyncState;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const KEYWORD_SEARCH_PAGE_SIZE: usize = 128;
const KEYWORD_SEARCH_MIN_SCAN_LIMIT: usize = 1_000;
const KEYWORD_SEARCH_MAX_SCAN_LIMIT: usize = 10_000;
const KEYWORD_SEARCH_SCAN_MULTIPLIER: usize = 8;
const LIST_FULL_TEXT_LIMIT: usize = 8192;
const LIST_RICH_HTML_LIMIT: usize = 4096;
const LIST_RICH_AUX_LIMIT: usize = 2048;
const LIST_NOTE_LIMIT: usize = 2048;

/// One page of keyword candidates. `exhausted` tracks the raw database page
/// independently of the filtered `matches`, so a page whose candidates were
/// all filtered out still advances the scan instead of being mistaken for
/// the end of data.
struct KeywordPage {
    /// Whether the raw database candidate page was empty (candidate data
    /// exhausted).
    exhausted: bool,
    /// Items that passed foreign-path and keyword filtering, in page order.
    matches: Vec<ClipboardItem>,
}

/// Root application state entity.
///
/// Held in an `Entity<AppState>` by the root view. Child views access it
/// via `cx.read_entity()` / `cx.update_entity()` or through observation.
pub struct AppState {
    /// Persisted settings (TOML-backed)
    pub settings: AppSettings,
    /// SQLite database
    pub db: Database,
    /// Clipboard items loaded from DB
    pub items: Vec<ClipboardItem>,
    /// In-memory image placeholders awaiting background persistence.
    /// Not persisted; consumed by the list view for immediate feedback.
    pub pending_images: Vec<crate::core::types::PendingImageView>,
    /// All tags loaded from DB
    pub tags: Vec<TagInfo>,
    /// Active filter state
    pub filters: ClipboardFilters,
    /// Whether any persisted item currently has a custom hotkey.
    pub has_hotkey_items: bool,
    /// Whether any persisted item is currently favorited.
    pub has_favorite_items: bool,
    /// Number of non-transfer clipboard rows affected by "clear data".
    pub clearable_history_count: u32,
    /// Number of non-favorite, non-transfer rows affected by the default clear action.
    pub clearable_non_favorite_history_count: u32,
    /// Whether remote transfer station files exist (controls titlebar button visibility).
    pub has_transfer_files: bool,
    /// Whether the transfer station filter/view is active.
    pub transfer_filter_active: bool,
    /// Resolved transfer entries (manifest entries with is_local status).
    /// Populated when `transfer_filter_active` is true, cleared when false.
    pub transfer_entries: Vec<crate::core::transfer_types::ResolvedEntry>,
    /// Commands consumed by the background transfer runtime.
    pub pending_transfer_commands: VecDeque<crate::services::transfer_station::TransferCommand>,
    /// Hashes of cloud entries queued for or currently being downloaded.
    /// Used to render per-entry progress feedback and suppress duplicate jobs.
    pub pending_transfer_downloads: HashSet<String>,
    /// Hashes with an in-flight pin/unpin command; duplicate clicks are ignored
    /// until the command completes (success or failure) and the hash is removed.
    pub pending_transfer_pin_updates: HashSet<String>,
    /// Source paths queued for or currently being uploaded.
    pub pending_transfer_uploads: HashSet<String>,
    /// Whether a transfer worker is currently running.
    pub transfer_busy: bool,
    /// Whether the active transfer worker is pulling the remote manifest.
    pub transfer_refreshing: bool,
    /// Currently selected item IDs (for batch operations)
    pub selected_ids: Vec<i64>,
    /// Tag editing state for TagFilterPanel overlay
    pub editing_tag_id: i64,
    pub editing_tag_name: String,
    pub editing_tag_color: String,
    /// Clipboard item editing state for the GPUI edit panel.
    pub editing_item_id: i64,
    pub editing_item: Option<ClipboardItem>,
    /// Shared with clipboard listener — set true during batch paste
    /// to prevent recording intermediate writes (newline separators).
    pub batch_pasting: Arc<AtomicBool>,
    /// Shared with clipboard listener — set true before writing OCR/color
    /// conversion text to clipboard. The listener skips one cycle and
    /// updates the baseline sequence number so the internal write is never
    /// detected as a new history entry (unlike batch_pasting which only
    /// provides a time window).
    pub skip_next: Arc<AtomicBool>,
    /// Shared with SyncManager — true when local data has changed.
    /// [FUTURE] When SyncManager is migrated to GPUI, pass this Arc to
    /// SyncManager::new() so it can detect local changes and trigger sync
    /// cycles. Tombstone recording (record_item_deletion, record_unfavorite,
    /// remove_unfavorite) is already handled in the data mutation methods
    /// below — SyncManager only needs to observe this flag.
    pub sync_dirty: Arc<AtomicBool>,
    pub bitmap_paste_finished: Arc<AtomicBool>,
    /// IDs touched by successful usage updates (copy/paste) since the list
    /// last consumed them. Accumulated so background hotkey paths cannot
    /// overwrite an earlier pending update.
    last_usage_touched_ids: Vec<i64>,
    /// Content changed while processing a usage action (for example, OCR was
    /// cached). The list must perform one full sync instead of copying only
    /// `updated_at`.
    usage_sync_requires_full_reload: bool,
    /// Item IDs whose custom hotkeys need unregistering (consumed by WindowManager poll).
    pub pending_hotkey_unregister: Vec<i64>,
    pub toast_message: Option<String>,
    /// true = warning (red), false = info (green).
    pub toast_is_warning: bool,
    /// Foreground app info (updated by WindowManager poll loop, consumed by hotkey settings tab).
    pub foreground_app_name: String,
    pub foreground_window_title: String,
    /// Base64-encoded PNG icon of the current foreground app.
    pub foreground_app_icon_base64: String,
    /// Whether a hotkey recording is in progress (set by settings UI, cleared by WM poll).
    pub hotkey_recording: bool,
    /// Whether a quick-window hotkey recording is in progress.
    pub recording_quick_hotkey: bool,
    /// Modifier-less key waiting for single-click confirmation or a second press.
    pub pending_single_hotkey: Option<String>,
    /// GPUI-facing sync status and backend snapshots.
    pub sync: SyncState,
    /// Update available info (set by WM poll, consumed by RootView + settings).
    pub update_available: Option<UpdateInfo>,
    /// Current update phase (for UI display).
    pub update_phase: UpdatePhase,
}

/// Pinyin-aware matching now lives in `core::search::text_matches_term` —
/// the single implementation shared with the transfer-station search and the
/// highlight renderer. This keeps filtering and highlighting in lockstep.
fn item_matches_keyword(item: &crate::core::types::ClipboardItem, keyword: &str) -> bool {
    let matches = |text: &str| crate::core::search::text_matches_term(text, keyword);
    let full_text_matches = if matches!(item.display_kind(), DisplayKind::Html) {
        html_text::has_visible_match(&item.full_text, matches)
    } else {
        matches(&item.full_text)
    };
    if full_text_matches {
        return true;
    }

    if !item.rich_data.is_empty() {
        let rich = RichData::from_json(&item.rich_data);

        if let Some(html) = rich.html.as_deref() {
            if html_text::has_visible_match(html, matches) {
                return true;
            }
        }

        let rich_texts = [
            rich.rtf.as_deref(),
            rich.ocr_text.as_deref(),
            rich.qr_text.as_deref(),
            rich.page_title.as_deref(),
        ];
        if rich_texts.into_iter().flatten().any(matches) {
            return true;
        }
    }

    if !item.note.is_empty() && matches(&item.note) {
        return true;
    }

    item.tags.iter().any(|tag| matches(&tag.name))
}

fn item_matches_keywords(item: &crate::core::types::ClipboardItem, keywords: &[String]) -> bool {
    keywords
        .iter()
        .all(|keyword| item_matches_keyword(item, keyword))
}

fn truncate_chars(text: &mut String, limit: usize) {
    if let Some((idx, _)) = text.char_indices().nth(limit) {
        text.truncate(idx);
    }
}

fn truncated_owned(text: String, limit: usize) -> String {
    let mut text = text;
    truncate_chars(&mut text, limit);
    text
}

fn item_can_be_merged(item: &ClipboardItem) -> bool {
    matches!(
        item.content_type,
        ContentType::PlainText | ContentType::RichText
    ) && !item.full_text.is_empty()
}

fn shrink_item_for_list(item: &mut ClipboardItem) {
    truncate_chars(&mut item.full_text, LIST_FULL_TEXT_LIMIT);
    truncate_chars(&mut item.note, LIST_NOTE_LIMIT);

    if item.rich_data.is_empty() {
        return;
    }

    let rich = RichData::from_json(&item.rich_data);
    let preview = RichData {
        html: rich
            .html
            .filter(|text| !text.is_empty())
            .map(|text| truncated_owned(text, LIST_RICH_HTML_LIMIT)),
        rtf: rich
            .rtf
            .filter(|text| !text.is_empty())
            .map(|text| truncated_owned(text, LIST_RICH_HTML_LIMIT)),
        ocr_text: rich
            .ocr_text
            .filter(|text| !text.is_empty())
            .map(|text| truncated_owned(text, LIST_RICH_AUX_LIMIT)),
        qr_text: rich
            .qr_text
            .filter(|text| !text.is_empty())
            .map(|text| truncated_owned(text, LIST_RICH_AUX_LIMIT)),
        page_title: rich
            .page_title
            .filter(|text| !text.is_empty())
            .map(|text| truncated_owned(text, LIST_RICH_AUX_LIMIT)),
        drive_label: rich
            .drive_label
            .filter(|text| !text.is_empty())
            .map(|text| truncated_owned(text, LIST_RICH_AUX_LIMIT)),
        remote_host: rich
            .remote_host
            .filter(|text| !text.is_empty())
            .map(|text| truncated_owned(text, LIST_RICH_AUX_LIMIT)),
    };
    item.rich_data = preview.to_json();
}

/// Stable reorder for usage-time updates, mirroring the database sort.
/// `created_at` ordering keeps positions; favorites-first keyword searches
/// reorder only inside each favorite group (all favorites lead the list).
fn reorder_by_usage(items: &mut [ClipboardItem], sort_by_created: bool, favorites_first: bool) {
    if sort_by_created || items.is_empty() {
        return;
    }
    if favorites_first {
        let split = items
            .iter()
            .position(|item| !item.is_favorite)
            .unwrap_or(items.len());
        items[..split].sort_by_key(|item| std::cmp::Reverse(item.updated_at));
        items[split..].sort_by_key(|item| std::cmp::Reverse(item.updated_at));
    } else {
        items.sort_by_key(|item| std::cmp::Reverse(item.updated_at));
    }
}

/// Deduplicate ids preserving first-occurrence order.
fn dedupe_ids(ids: &[i64]) -> Vec<i64> {
    let mut seen = std::collections::HashSet::new();
    ids.iter().copied().filter(|id| seen.insert(*id)).collect()
}

impl AppState {
    /// Open the database and load initial data.
    pub fn new(settings: AppSettings) -> Self {
        let db_path = settings.resolve_db_path();
        crate::core::paths::init_images_dir(&settings.db_path);
        let order_by = if settings.sort_by_created {
            "created_at"
        } else {
            "updated_at"
        };
        let query_limit = if settings.max_items == 0 {
            200
        } else {
            (settings.max_items as usize).saturating_mul(2).max(200)
        };
        let db = Database::open(&db_path.to_string_lossy())
            .unwrap_or_else(|e| panic!("Failed to open database at {db_path:?}: {e}"));
        // Startup cleanup is deferred to the first WindowManager poll tick
        // which checks cleanup_last_date / retention_cleanup_last_date and
        // schedules the work on a background thread.
        let initial_cleanup_dirty = false;

        let items = db
            .load_filtered_list_with_tags(&ClipboardFilters::default(), query_limit, order_by)
            .unwrap_or_else(|e| {
                log::error!("Failed to load initial items: {e}");
                Vec::new()
            });

        let tags = db.get_all_tags().unwrap_or_else(|e| {
            log::error!("Failed to load tags: {e}");
            Vec::new()
        });

        let sync = SyncState::from_settings(&settings);
        let stats = db.load_titlebar_stats().unwrap_or_else(|e| {
            log::error!("Failed to load titlebar stats: {e}");
            crate::core::db::TitlebarStats::default()
        });

        Self {
            settings,
            db,
            items,
            pending_images: Vec::new(),
            tags,
            filters: ClipboardFilters::default(),
            last_usage_touched_ids: Vec::new(),
            usage_sync_requires_full_reload: false,
            has_hotkey_items: stats.has_hotkey_items,
            has_favorite_items: stats.has_favorite_items,
            clearable_history_count: stats.clearable_history_count,
            clearable_non_favorite_history_count: stats.clearable_non_favorite_history_count,
            has_transfer_files: false,
            transfer_filter_active: false,
            transfer_entries: Vec::new(),
            pending_transfer_commands: VecDeque::new(),
            pending_transfer_downloads: HashSet::new(),
            pending_transfer_pin_updates: HashSet::new(),
            pending_transfer_uploads: HashSet::new(),
            transfer_busy: false,
            transfer_refreshing: false,
            selected_ids: Vec::new(),
            editing_tag_id: -1,
            editing_tag_name: String::new(),
            editing_tag_color: "#3B82F6".into(),
            editing_item_id: -1,
            editing_item: None,
            batch_pasting: Arc::new(AtomicBool::new(false)),
            skip_next: Arc::new(AtomicBool::new(false)),
            sync_dirty: Arc::new(AtomicBool::new(initial_cleanup_dirty)),
            pending_hotkey_unregister: Vec::new(),
            bitmap_paste_finished: Arc::new(AtomicBool::new(false)),
            toast_message: None,
            toast_is_warning: false,
            foreground_app_name: String::new(),
            foreground_window_title: String::new(),
            foreground_app_icon_base64: String::new(),
            hotkey_recording: false,
            recording_quick_hotkey: false,
            pending_single_hotkey: None,
            sync,
            update_available: None,
            update_phase: UpdatePhase::Idle,
        }
    }

    /// Check whether a mutation on this item should mark sync as dirty.
    ///
    /// File items are never synced. Image items are synced only when image
    /// sync is enabled. In favorites-only mode, only favorite items trigger
    /// sync cycles.
    pub fn should_mark_sync_dirty(&self, item: &ClipboardItem) -> bool {
        crate::core::sync_scope::item_in_sync_scope(
            item.content_type,
            item.is_favorite,
            self.settings.sync_include_images,
            self.settings.sync_favorites_only,
        )
    }

    fn refresh_titlebar_filter_availability(&mut self) {
        match self.db.load_titlebar_stats() {
            Ok(stats) => {
                self.has_hotkey_items = stats.has_hotkey_items;
                self.has_favorite_items = stats.has_favorite_items;
                self.clearable_history_count = stats.clearable_history_count;
                self.clearable_non_favorite_history_count =
                    stats.clearable_non_favorite_history_count;
            }
            Err(e) => log::error!("Failed to refresh titlebar stats: {e}"),
        }
    }

    /// Reload items from database with current filters.
    pub fn reload_items(&mut self) {
        let result = if self.filters.has_keyword() {
            self.load_keyword_filtered_items()
        } else {
            self.db
                .load_filtered_list_with_tags(&self.filters, self.query_limit(), self.order_by())
        };

        let keyword_filtered = self.filters.has_keyword();

        match result {
            Ok(mut items) => {
                let keywords = self.filters.keyword_terms();
                if !keyword_filtered && !keywords.is_empty() {
                    items.retain(|item| item_matches_keywords(item, &keywords));
                }
                // Hide non-native platform paths when the setting is enabled.
                if self.settings.filter_foreign_paths {
                    items.retain(|item| {
                        if item.meta_type == "path" {
                            crate::core::types::path_is_native(&item.full_text)
                        } else {
                            true
                        }
                    });
                }
                self.items = items;
            }
            Err(e) => log::error!("Failed to reload items: {e}"),
        }
        self.refresh_titlebar_filter_availability();
    }

    /// Number of non-transfer history rows affected by the data-page
    /// "clear data" action. The cached value is refreshed alongside other
    /// titlebar/filter availability metadata, never queried from render().
    pub fn clearable_history_count(&self) -> u32 {
        self.clearable_history_count
    }

    pub fn clearable_non_favorite_history_count(&self) -> u32 {
        self.clearable_non_favorite_history_count
    }

    fn load_keyword_filtered_items(&self) -> rusqlite::Result<Vec<ClipboardItem>> {
        let keywords = self.filters.keyword_terms();
        if keywords.is_empty() {
            return self.db.load_filtered_with_tags(
                &self.filters,
                self.query_limit(),
                self.order_by(),
            );
        }

        let result_limit = self.query_limit();
        let scan_limit = result_limit
            .saturating_mul(KEYWORD_SEARCH_SCAN_MULTIPLIER)
            .clamp(KEYWORD_SEARCH_MIN_SCAN_LIMIT, KEYWORD_SEARCH_MAX_SCAN_LIMIT);

        // Favorites-first only reorders keyword searches. With the favorites-only
        // filter every result is already a favorite, so the single-bucket path
        // below stays active and unchanged.
        let prioritize =
            self.settings.search_favorites_first && !self.filters.is_favorites_active();

        if !prioritize {
            // Single-bucket path (existing behavior): stop as soon as the result
            // limit is reached, so disabled mode has no performance regression.
            let mut matches = Vec::new();
            let mut offset = 0;

            while offset < scan_limit && matches.len() < result_limit {
                let page_limit = KEYWORD_SEARCH_PAGE_SIZE.min(scan_limit - offset);
                let page = self.load_keyword_page(&keywords, page_limit, offset)?;
                if page.exhausted {
                    break;
                }
                matches.extend(page.matches.into_iter().take(result_limit - matches.len()));
                if page_limit < KEYWORD_SEARCH_PAGE_SIZE {
                    break;
                }
                offset += page_limit;
            }

            return Ok(matches);
        }

        // Favorites-first path: keep scanning until the favorite bucket is full,
        // so older favorite matches are still discovered after the regular bucket
        // reaches its limit. Both buckets preserve the database time order.
        let mut favorite_matches = Vec::new();
        let mut regular_matches = Vec::new();
        let mut offset = 0;

        'pages: while offset < scan_limit && favorite_matches.len() < result_limit {
            let page_limit = KEYWORD_SEARCH_PAGE_SIZE.min(scan_limit - offset);
            let page = self.load_keyword_page(&keywords, page_limit, offset)?;
            if page.exhausted {
                break;
            }

            for item in page.matches {
                if item.is_favorite {
                    if favorite_matches.len() < result_limit {
                        favorite_matches.push(item);
                    }
                } else if regular_matches.len() < result_limit {
                    regular_matches.push(item);
                }

                // Later candidates are older, so once favorites fill the result
                // limit no later item can enter the final list.
                if favorite_matches.len() >= result_limit {
                    break 'pages;
                }
            }

            if page_limit < KEYWORD_SEARCH_PAGE_SIZE {
                break;
            }
            offset += page_limit;
        }

        favorite_matches.extend(regular_matches);
        favorite_matches.truncate(result_limit);
        Ok(favorite_matches)
    }

    /// Load one page of keyword candidates with the shared pagination, foreign
    /// path filtering, in-memory keyword matching and list preview shrinking
    /// rules. Both the single-bucket and the favorites-first search paths reuse
    /// this so their candidate semantics stay in lockstep.
    ///
    /// `KeywordPage::exhausted` reflects the raw database page, so an
    /// all-non-matching page advances the scan instead of being mistaken for
    /// the end of data.
    fn load_keyword_page(
        &self,
        keywords: &[String],
        page_limit: usize,
        offset: usize,
    ) -> rusqlite::Result<KeywordPage> {
        let mut page = self.db.load_filtered_page_with_tags(
            &self.filters,
            page_limit,
            offset,
            self.order_by(),
        )?;
        let exhausted = page.is_empty();
        if self.settings.filter_foreign_paths {
            page.retain(|item| {
                item.meta_type != "path" || crate::core::types::path_is_native(&item.full_text)
            });
        }
        page.retain(|item| item_matches_keywords(item, keywords));
        for item in &mut page {
            shrink_item_for_list(item);
        }
        Ok(KeywordPage {
            exhausted,
            matches: page,
        })
    }

    /// Clear all items from memory to free resources while window is hidden.
    /// Items are reloaded from DB on next `reload_items()` call.
    pub fn clear_items(&mut self) {
        self.items.clear();
        self.items.shrink_to_fit();
        self.selected_ids.clear();
    }

    /// Update keyword filter and reload visible items.
    pub fn set_keyword(&mut self, keyword: &str) {
        self.filters.set_keyword(keyword);
        self.selected_ids.clear();
        if self.transfer_filter_active {
            // The transfer view filters the in-memory manifest inside
            // `visible_items()`; never touch the DB per keystroke here.
            return;
        }
        self.reload_items();
    }

    /// Toggle a content-type filter and reload visible items.
    /// Each type filter is now independent (image/file are separate).
    pub fn toggle_type_filter(&mut self, type_name: &str) {
        self.filters.toggle_type(type_name);
        self.selected_ids.clear();
        self.reload_items();
    }

    /// Toggle favorites-only filter and reload visible items.
    pub fn toggle_favorites_filter(&mut self) {
        self.filters.toggle_favorites_only();
        self.selected_ids.clear();
        self.reload_items();
    }

    /// Toggle hotkeys-only filter and reload visible items.
    pub fn toggle_hotkeys_filter(&mut self) {
        self.filters.toggle_hotkeys_only();
        self.selected_ids.clear();
        self.reload_items();
    }

    /// Toggle a tag filter and reload visible items.
    pub fn toggle_tag_filter(&mut self, tag_id: i64) {
        self.filters.toggle_tag(tag_id);
        self.selected_ids.clear();
        self.reload_items();
    }

    /// Clear tag filters and reload visible items.
    pub fn clear_tag_filters(&mut self) {
        self.filters.clear_tag_filters();
        self.selected_ids.clear();
        self.reload_items();
    }

    /// Toggle tag matching mode and reload visible items.
    pub fn toggle_tag_match_mode(&mut self) {
        self.filters.toggle_tag_mode();
        self.selected_ids.clear();
        self.reload_items();
    }

    /// Toggle whether a tag is pinned in the side tag bar, then persist settings.
    pub fn toggle_pinned_tag(&mut self, tag_id: i64) {
        if let Some(pos) = self
            .settings
            .pinned_tag_ids
            .iter()
            .position(|&id| id == tag_id)
        {
            self.settings.pinned_tag_ids.remove(pos);
        } else {
            self.settings.pinned_tag_ids.push(tag_id);
        }
        self.settings.save();
    }

    /// Reload tags from database.
    pub fn reload_tags(&mut self) {
        match self.db.get_all_tags() {
            Ok(tags) => self.tags = tags,
            Err(e) => log::error!("Failed to reload tags: {e}"),
        }
    }

    /// Select a single item, clearing previous selection.
    pub fn select_single(&mut self, id: i64) {
        self.selected_ids.clear();
        self.selected_ids.push(id);
    }

    /// Replace selection with a range of IDs (for Ctrl+click toggle and Shift+click range).
    pub fn range_select(&mut self, ids: &[i64]) {
        self.selected_ids = ids.to_vec();
    }

    /// Clear all selections.
    pub fn clear_selection(&mut self) {
        self.selected_ids.clear();
    }

    /// Start editing a tag (opens TagEditPanel overlay).
    pub fn start_edit_tag(&mut self, id: i64, name: &str, color: &str) {
        self.editing_tag_id = id;
        self.editing_tag_name = name.to_string();
        self.editing_tag_color = color.to_string();
    }

    /// Cancel tag editing.
    pub fn cancel_edit_tag(&mut self) {
        self.editing_tag_id = -1;
        self.editing_tag_name.clear();
    }

    /// Update the preview color during tag editing (no save).
    pub fn set_edit_tag_color(&mut self, color: &str) {
        self.editing_tag_color = color.to_string();
    }

    /// Create a new tag with round-robin color from presets.
    pub fn create_tag(&mut self, name: &str) {
        let count = self.tags.len();
        let color = next_tag_color(count);
        match self.db.create_tag(name, color) {
            Ok(_) => {
                self.sync_dirty.store(true, Ordering::SeqCst);
                self.reload_tags();
            }
            Err(e) => log::error!("Failed to create tag: {e}"),
        }
    }

    /// Delete a tag by id. Returns `true` on success so the UI can close its
    /// confirmation dialog only after the deletion actually happened.
    pub fn delete_tag(&mut self, tag_id: i64) -> bool {
        let tag = match self.db.get_tag_by_id(tag_id) {
            Ok(tag) => tag,
            Err(e) => {
                log::error!("Failed to load tag before deletion: {e}");
                None
            }
        };
        match self.db.delete_tag(tag_id) {
            Ok(_) => {
                if let Some(tag) = tag {
                    let now = chrono::Utc::now().to_rfc3339();
                    let device = crate::services::backends::local_folder::hostname();
                    if let Err(e) = self
                        .db
                        .record_tag_deletion(&tag.uid, &tag.name, &now, &device)
                    {
                        log::error!("Failed to record tag deletion: {e}");
                    }
                }
                self.sync_dirty.store(true, Ordering::SeqCst);
                self.filters.tag_ids.retain(|&id| id != tag_id);
                // Remove any stale pinned sidebar entry for the deleted tag.
                self.settings.pinned_tag_ids.retain(|&id| id != tag_id);
                self.settings.save();
                self.reload_tags();
                self.reload_items();
                true
            }
            Err(e) => {
                log::error!("Failed to delete tag: {e}");
                false
            }
        }
    }

    /// Update tag name and color.
    pub fn update_tag(&mut self, tag_id: i64, name: &str, color: &str) {
        match self.db.update_tag(tag_id, name, color) {
            Ok(_) => {
                self.sync_dirty.store(true, Ordering::SeqCst);
                self.cancel_edit_tag();
                self.reload_tags();
                // Incremental update: update tag name/color on all items
                // in-place so clipboard cards show the new values immediately
                // without reloading from DB (which would reorder by updated_at).
                for item in &mut self.items {
                    if let Some(tag) = item.tags.iter_mut().find(|t| t.id == tag_id) {
                        tag.name = name.to_string();
                        tag.color = color.to_string();
                    }
                }
            }
            Err(e) => log::error!("Failed to update tag: {e}"),
        }
    }

    pub fn start_edit_item(&mut self, id: i64) -> bool {
        match self.db.get_by_id_with_tags(id) {
            Ok(Some(item)) => {
                self.editing_item_id = id;
                self.editing_item = Some(item);
                true
            }
            Ok(None) => {
                log::warn!("start_edit_item({id}): item not found");
                false
            }
            Err(e) => {
                log::error!("start_edit_item({id}): {e}");
                false
            }
        }
    }

    pub fn cancel_edit_item(&mut self) {
        self.editing_item_id = -1;
        self.editing_item = None;
    }

    pub fn save_edited_item(&mut self, id: i64, text: &str, editor_type: &str) -> bool {
        let (content_type, meta_type, rich_data) = Self::storage_for_editor_type(editor_type, text);
        // Pre-flight: is the item in sync scope? Check before DB write.
        let mark_dirty = self
            .items
            .iter()
            .find(|it| it.id == id)
            .is_some_and(|item| self.should_mark_sync_dirty(item));
        match self
            .db
            .update_content_with_rich_data(id, text, content_type, meta_type, &rich_data)
        {
            Ok(_) => {
                if mark_dirty {
                    self.sync_dirty.store(true, Ordering::SeqCst);
                }
                self.cancel_edit_item();
                // Incremental update: preserve scroll position (consistent with
                // update_note / toggle_favorite). The item keeps its current
                // position; re-sort happens on next window open via reload_items().
                if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                    item.full_text = text.to_string();
                    item.content_type = ContentType::from_str(content_type);
                    item.meta_type = meta_type.to_string();
                    item.rich_data = rich_data;
                    item.image_path.clear();
                    item.file_data.clear();
                    item.image_width = 0;
                    item.image_height = 0;
                    item.size = text.chars().count() as i64;
                    item.updated_at = chrono::Utc::now();
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    std::hash::Hash::hash(&text, &mut hasher);
                    item.content_hash = std::hash::Hasher::finish(&hasher);
                }
                true
            }
            Err(e) => {
                log::error!("save_edited_item({id}): {e}");
                false
            }
        }
    }

    fn storage_for_editor_type(
        editor_type: &str,
        text: &str,
    ) -> (&'static str, &'static str, String) {
        match editor_type {
            "markdown" => ("rich_text", "markdown", String::new()),
            "html" => {
                let rich = RichData {
                    html: Some(text.to_string()),
                    ..Default::default()
                };
                ("rich_text", "html", rich.to_json())
            }
            "link" => ("plain_text", "link", String::new()),
            "path" => ("plain_text", "path", String::new()),
            "color" => ("plain_text", "color", String::new()),
            "email" => ("plain_text", "email", String::new()),
            "phone" => ("plain_text", "phone", String::new()),
            "secret" => ("plain_text", "secret", String::new()),
            _ => ("plain_text", "", String::new()),
        }
    }

    pub fn clear_toast(&mut self) {
        self.toast_message = None;
        self.toast_is_warning = false;
    }

    pub fn show_toast(&mut self, message: impl Into<String>) {
        self.toast_message = Some(message.into());
        self.toast_is_warning = false;
    }

    pub fn show_warning_toast(&mut self, message: impl Into<String>) {
        self.toast_message = Some(message.into());
        self.toast_is_warning = true;
    }

    pub fn take_bitmap_paste_finished(&self) -> bool {
        self.bitmap_paste_finished.swap(false, Ordering::SeqCst)
    }

    /// Touch the usage time of one or more items: one database transaction
    /// for the whole set, then a single in-memory reorder. Callers pass the
    /// full items already read for the copy/paste operation (no re-read by
    /// ID); the in-memory update never inserts items that are not part of
    /// the current filter result.
    fn touch_items_usage(&mut self, items: &[ClipboardItem]) {
        if items.is_empty() {
            return;
        }
        let mark_dirty = items.iter().any(|item| self.should_mark_sync_dirty(item));
        let now = chrono::Utc::now().to_rfc3339();
        let ids = dedupe_ids(&items.iter().map(|item| item.id).collect::<Vec<_>>());
        match self.db.touch_items(&ids, &now) {
            Ok(_) => {
                if mark_dirty {
                    self.sync_dirty.store(true, Ordering::SeqCst);
                }
                self.apply_usage_touch_in_memory(&ids, &now);
                for id in ids {
                    if !self.last_usage_touched_ids.contains(&id) {
                        self.last_usage_touched_ids.push(id);
                    }
                }
            }
            Err(e) => log::error!("touch_items_usage: {e}"),
        }
    }

    fn touch_item_usage(&mut self, item: &ClipboardItem) {
        self.touch_items_usage(std::slice::from_ref(item));
    }

    /// In-memory half of a usage-time update: refresh `updated_at` for the
    /// touched ids that are currently visible and reorder once. Items outside
    /// the current filter result are only updated in the database.
    fn apply_usage_touch_in_memory(&mut self, ids: &[i64], now: &str) {
        let ts = chrono::DateTime::parse_from_rfc3339(now)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());
        let mut hit = false;
        for id in ids {
            if let Some(item) = self.items.iter_mut().find(|item| item.id == *id) {
                item.updated_at = ts;
                hit = true;
            }
        }
        if hit {
            let favorites_first = self.settings.search_favorites_first
                && self.filters.has_keyword()
                && !self.filters.is_favorites_active();
            reorder_by_usage(
                &mut self.items,
                self.settings.sort_by_created,
                favorites_first,
            );
        }
    }

    /// Consume the pending list-sync request. A full reload supersedes the
    /// accumulated IDs when the action also changed item content.
    pub(crate) fn take_usage_sync_request(&mut self) -> (Vec<i64>, bool) {
        (
            std::mem::take(&mut self.last_usage_touched_ids),
            std::mem::take(&mut self.usage_sync_requires_full_reload),
        )
    }

    /// A full list sync already includes every pending usage update.
    pub(crate) fn clear_usage_sync_request(&mut self) {
        self.last_usage_touched_ids.clear();
        self.usage_sync_requires_full_reload = false;
    }

    pub fn toggle_item_tag(&mut self, item_id: i64, tag_id: i64) {
        let item = match self.items.iter().find(|item| item.id == item_id) {
            Some(item) => item,
            None => return,
        };
        let mark_dirty = self.should_mark_sync_dirty(item);
        let has_tag = item.tags.iter().any(|tag| tag.id == tag_id);
        let result = if has_tag {
            self.db.remove_item_tag(item_id, tag_id)
        } else {
            self.db.add_item_tag(item_id, tag_id)
        };
        if let Err(e) = result {
            log::error!("toggle_item_tag({item_id}, {tag_id}): {e}");
            return;
        }
        if mark_dirty {
            self.sync_dirty.store(true, Ordering::SeqCst);
        }
        // Incremental update: re-fetch tags for the affected item instead of
        // reloading all items, so scroll position is preserved.
        if let Ok(Some(updated)) = self.db.get_by_id_with_tags(item_id) {
            if let Some(item) = self.items.iter_mut().find(|it| it.id == item_id) {
                item.tags = updated.tags;
                item.updated_at = updated.updated_at;
            }
        }
    }

    pub fn batch_add_tag(&mut self, ids: &[i64], tag_id: i64) {
        let mark_dirty = ids.iter().any(|&id| {
            self.items
                .iter()
                .find(|it| it.id == id)
                .is_some_and(|item| self.should_mark_sync_dirty(item))
        });
        for &id in ids {
            if let Err(e) = self.db.add_item_tag(id, tag_id) {
                log::error!("batch_add_tag({id}, {tag_id}): {e}");
            }
        }
        if mark_dirty {
            self.sync_dirty.store(true, Ordering::SeqCst);
        }
        // Incremental update: re-fetch tags for affected items (preserve scroll position)
        for &id in ids {
            if let Ok(Some(updated)) = self.db.get_by_id_with_tags(id) {
                if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                    item.tags = updated.tags;
                    item.updated_at = updated.updated_at;
                }
            }
        }
    }

    pub fn batch_remove_tag(&mut self, ids: &[i64], tag_id: i64) {
        let mark_dirty = ids.iter().any(|&id| {
            self.items
                .iter()
                .find(|it| it.id == id)
                .is_some_and(|item| self.should_mark_sync_dirty(item))
        });
        for &id in ids {
            if let Err(e) = self.db.remove_item_tag(id, tag_id) {
                log::error!("batch_remove_tag({id}, {tag_id}): {e}");
            }
        }
        if mark_dirty {
            self.sync_dirty.store(true, Ordering::SeqCst);
        }
        // Incremental update: re-fetch tags for affected items (preserve scroll position)
        for &id in ids {
            if let Ok(Some(updated)) = self.db.get_by_id_with_tags(id) {
                if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                    item.tags = updated.tags;
                    item.updated_at = updated.updated_at;
                }
            }
        }
    }

    pub fn clear_item_tags(&mut self, item_id: i64) {
        let mark_dirty = self
            .items
            .iter()
            .find(|it| it.id == item_id)
            .is_some_and(|item| self.should_mark_sync_dirty(item));
        if let Err(e) = self.db.clear_item_tags(item_id) {
            log::error!("clear_item_tags({item_id}): {e}");
            return;
        }
        if mark_dirty {
            self.sync_dirty.store(true, Ordering::SeqCst);
        }
        // Incremental update: re-fetch tags for the affected item (preserve scroll position)
        if let Ok(Some(updated)) = self.db.get_by_id_with_tags(item_id) {
            if let Some(item) = self.items.iter_mut().find(|it| it.id == item_id) {
                item.tags = updated.tags;
                item.updated_at = updated.updated_at;
            }
        }
    }

    pub fn clear_tags_for_items(&mut self, ids: &[i64]) {
        let mark_dirty = ids.iter().any(|&id| {
            self.items
                .iter()
                .find(|it| it.id == id)
                .is_some_and(|item| self.should_mark_sync_dirty(item))
        });
        for &id in ids {
            if let Err(e) = self.db.clear_item_tags(id) {
                log::error!("clear_tags_for_items({id}): {e}");
            }
        }
        if mark_dirty {
            self.sync_dirty.store(true, Ordering::SeqCst);
        }
        // Incremental update: re-fetch tags for affected items (preserve scroll position)
        for &id in ids {
            if let Ok(Some(updated)) = self.db.get_by_id_with_tags(id) {
                if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                    item.tags = updated.tags;
                    item.updated_at = updated.updated_at;
                }
            }
        }
    }

    fn order_by(&self) -> &'static str {
        if self.settings.sort_by_created {
            "created_at"
        } else {
            "updated_at"
        }
    }

    fn query_limit(&self) -> usize {
        if self.settings.max_items == 0 {
            200
        } else {
            (self.settings.max_items as usize)
                .saturating_mul(2)
                .max(200)
        }
    }

    pub fn open_original_image(&self, id: i64) {
        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("open_original_image: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("open_original_image: db error for {id}: {e}");
                return;
            }
        };
        if !item.image_path.is_empty() {
            let path = item.image_path.clone();
            // --- Spawn on a background thread — ShellExecuteW can pump Windows ---
            // messages internally (DDE/COM) and deadlock if called from the
            // --- GPUI main thread event handler. ---
            std::thread::spawn(move || {
                open_system_target(&path);
            });
        }
    }

    /// Paste the image file path as plain text.
    pub fn paste_image_path(&mut self, id: i64) {
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("paste_image_path: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("paste_image_path: db error for {id}: {e}");
                return;
            }
        };
        if item.image_path.is_empty() {
            return;
        }
        self.touch_item_usage(&item);
        self.write_text_to_clipboard_internal(&item.image_path);
        restore_paste_target();
        let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
        paste_after_delay(shortcuts);
    }

    /// Paste the file paths as plain text.
    pub fn paste_file_path(&mut self, id: i64) {
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("paste_file_path: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("paste_file_path: db error for {id}: {e}");
                return;
            }
        };
        if item.file_data.is_empty() {
            return;
        }
        let file_data = FileData::from_json(&item.file_data);
        if file_data.files.is_empty() {
            return;
        }
        let paths_text: String = file_data
            .files
            .iter()
            .map(|f| f.path.clone())
            .collect::<Vec<_>>()
            .join("\n");

        self.touch_item_usage(&item);
        self.write_text_to_clipboard_internal(&paths_text);
        restore_paste_target();
        let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
        paste_after_delay(shortcuts);
    }

    pub fn open_item_location(&self, id: i64) {
        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("open_item_location: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("open_item_location: db error for {id}: {e}");
                return;
            }
        };

        // ── Link: open in browser ─────────────────────────────────
        if item.meta_type == "link" {
            if item.full_text.is_empty() {
                log::warn!("open_item_location: link item {id} has empty full_text");
                return;
            }
            let mut text = item.full_text.clone();
            // Protocol-less URLs need a scheme for ShellExecuteW to
            // dispatch them to the browser instead of treating them as
            // file-system paths.
            if !text.starts_with("http://") && !text.starts_with("https://") {
                text = format!("https://{}", text);
            }
            let target = text;

            // ── Lazy URL metadata backfill for old/synced link items ─────
            let mark_sync_dirty = self.should_mark_sync_dirty(&item);
            crate::services::url_assets::spawn_link_open_backfill(
                item.full_text.clone(),
                item.content_hash,
                self.settings.db_path.clone(),
                item.rich_data.clone(),
                self.settings.auto_fetch_url_title,
                self.sync_dirty.clone(),
                mark_sync_dirty,
            );

            // --- Spawn on a background thread to avoid ShellExecuteW ---
            // --- deadlock on the GPUI main thread (DDE/COM message pumping). ---
            std::thread::spawn(move || {
                open_system_target(&target);
            });
            return;
        }

        // ── Path: reveal in Explorer (not open/execute) ──────────
        if item.meta_type == "path" {
            if item.full_text.is_empty() {
                log::warn!("open_item_location: path item {id} has empty full_text");
                return;
            }

            // ── Lazy drive-label fill for old path items ──────────
            let rd = RichData::from_json(&item.rich_data);
            if rd.drive_label.is_none() {
                if let Some(label) = crate::core::types::path_drive_label(&item.full_text) {
                    let resolved = crate::core::paths::resolve_db_path(&self.settings.db_path);
                    if let Ok(db) = crate::core::db::Database::open(&resolved.to_string_lossy()) {
                        let mut new_rd = rd;
                        new_rd.drive_label = Some(label);
                        let _ = db.update_rich_data(item.id, &new_rd.to_json());
                    }
                }
            }

            // Reveal in Explorer if the path exists locally;
            // silently skip non-existent paths.
            if crate::core::types::path_exists(&item.full_text) {
                let path = item.full_text.clone();
                std::thread::spawn(move || {
                    reveal_file_location(&path);
                });
            }
            return;
        }

        // ── File list: reveal first file in Explorer ─────────────────
        if item.content_type == ContentType::File {
            let file_data = FileData::from_json(&item.file_data);
            if let Some(first) = file_data.files.first() {
                let path = first.path.clone();
                // --- Spawn on a background thread to avoid ShellExecuteW ---
                // --- deadlock on the GPUI main thread (DDE/COM message pumping). ---
                std::thread::spawn(move || {
                    reveal_file_location(&path);
                });
            }
            return;
        }

        log::warn!(
            "open_item_location: item {id} has no openable location \
             (content_type={:?}, meta_type={:?})",
            item.content_type,
            item.meta_type,
        );
    }

    pub fn qr_action(&mut self, id: i64) {
        let qr_text = match self.db.get_by_id(id) {
            Ok(Some(item)) => RichData::from_json(&item.rich_data).qr_text,
            Ok(None) => {
                log::warn!("qr_action: item {id} not found");
                None
            }
            Err(e) => {
                log::error!("qr_action: db error for {id}: {e}");
                None
            }
        };

        if let Some(text) = qr_text {
            self.handle_qr_text(text);
        } else {
            self.show_toast(I18nKey::ToastNoQr.text());
        }
    }

    pub fn qr_detect(&mut self, id: i64) {
        let qr_text = match self.db.get_by_id(id) {
            Ok(Some(item)) => {
                let rich = RichData::from_json(&item.rich_data);
                if rich.qr_text.is_some() {
                    rich.qr_text
                } else if !item.image_path.is_empty() {
                    match crate::core::qr::detect_qr(std::path::Path::new(&item.image_path)) {
                        Ok(Some(text)) => {
                            let mut next_rich = RichData::from_json(&item.rich_data);
                            next_rich.qr_text = Some(text.clone());
                            if let Err(e) = self.db.update_rich_data(id, &next_rich.to_json()) {
                                log::error!("qr_detect: update rich_data failed for {id}: {e}");
                            }
                            self.reload_items();
                            Some(text)
                        }
                        Ok(None) => None,
                        Err(e) => {
                            log::error!("qr_detect: {e}");
                            None
                        }
                    }
                } else {
                    None
                }
            }
            Ok(None) => {
                log::warn!("qr_detect: item {id} not found");
                None
            }
            Err(e) => {
                log::error!("qr_detect: db error for {id}: {e}");
                None
            }
        };

        if let Some(text) = qr_text {
            self.handle_qr_text(text);
        } else {
            self.show_toast(I18nKey::ToastNoQr.text());
        }
    }

    pub fn paste_ocr(&mut self, id: i64) {
        // Keep the full item for the usage-time update below; the OCR load
        // tuple only carries what the recognition path needs.
        let (ocr_load, item) = match self.db.get_by_id(id) {
            Ok(Some(item)) => {
                let rich = RichData::from_json(&item.rich_data);
                let load = match rich.ocr_text.filter(|text| !text.trim().is_empty()) {
                    Some(cached) => Some((cached, true, String::new(), String::new())),
                    None if item.image_path.is_empty() => None,
                    None => Some((
                        String::new(),
                        false,
                        item.image_path.clone(),
                        item.rich_data.clone(),
                    )),
                };
                (load, Some(item))
            }
            Ok(None) => {
                log::warn!("paste_ocr: item {id} not found");
                (None, None)
            }
            Err(e) => {
                log::error!("paste_ocr: db error for {id}: {e}");
                (None, None)
            }
        };

        let ocr_text = match ocr_load {
            Some((cached, true, _, _)) => Some(cached),
            Some((_, false, img_path, existing_rich)) => {
                let engine = crate::core::ocr::create_ocr_engine();
                match engine.recognize(std::path::Path::new(&img_path)) {
                    Ok(text) if !text.trim().is_empty() => {
                        let mut next_rich = RichData::from_json(&existing_rich);
                        next_rich.ocr_text = Some(text.clone());
                        match self.db.update_rich_data(id, &next_rich.to_json()) {
                            Ok(_) => {
                                // OCR changes the card payload and potentially
                                // its measured height. Do not reuse the stale
                                // incremental list cache after this action.
                                self.usage_sync_requires_full_reload = true;
                            }
                            Err(e) => {
                                log::error!("paste_ocr: update rich_data failed for {id}: {e}");
                            }
                        }
                        self.reload_items();
                        Some(text)
                    }
                    Ok(_) => None,
                    Err(e) => {
                        log::error!("OCR error for item {id}: {e}");
                        None
                    }
                }
            }
            _ => None,
        };

        if let Some(text) = ocr_text {
            // --- Use skip_next (not batch_pasting) — the OCR text written to ---
            // --- clipboard is internal and should be "consumed" by the listener ---
            // (skip one cycle + update baseline seq#) rather than recorded
            // as a new history entry. This matches the Slint-era behaviour.
            if let Some(item) = item.as_ref() {
                self.touch_item_usage(item);
            }
            self.write_text_to_clipboard_internal(&text);
            crate::platform::paste::restore_paste_target();
            let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
            crate::platform::paste::paste_after_delay(shortcuts);
        } else {
            self.show_toast(I18nKey::ToastNoOcr.text());
        }
    }

    fn handle_qr_text(&mut self, text: String) {
        if text.starts_with("http://") || text.starts_with("https://") {
            // --- Spawn on a background thread — open_releases_page calls ---
            // --- ShellExecuteW which can pump Windows messages internally ---
            // (DDE/COM) and deadlock if called from the GPUI main thread.
            let url = text;
            std::thread::spawn(move || {
                crate::services::update::open_releases_page(&url);
            });
            return;
        }
        if self.write_text_to_clipboard_internal(&text) {
            self.show_toast(I18nKey::ToastQrCopied.text());
        }
    }

    fn with_internal_clipboard_write<T>(
        batch_pasting: &Arc<AtomicBool>,
        skip_next: &Arc<AtomicBool>,
        operation: impl FnOnce() -> T,
    ) -> T {
        batch_pasting.store(true, Ordering::SeqCst);
        let result = operation();
        skip_next.store(true, Ordering::SeqCst);
        batch_pasting.store(false, Ordering::SeqCst);
        result
    }

    pub(crate) fn write_item_to_clipboard_internal(
        &self,
        item: &ClipboardItem,
        copy_as_plain_text: bool,
    ) {
        Self::with_internal_clipboard_write(&self.batch_pasting, &self.skip_next, || {
            crate::services::clipboard_ops::write_item_to_clipboard(item, copy_as_plain_text);
        });
    }

    fn write_item_as_plain_text_to_clipboard_internal(&self, item: &ClipboardItem) {
        Self::with_internal_clipboard_write(&self.batch_pasting, &self.skip_next, || {
            crate::services::clipboard_ops::write_item_as_plain_text_to_clipboard(item);
        });
    }

    fn write_text_to_clipboard_internal(&self, text: &str) -> bool {
        Self::with_internal_clipboard_write(&self.batch_pasting, &self.skip_next, || {
            crate::services::clipboard_ops::write_text_to_clipboard(text)
        })
    }

    /// Copy a single item to the system clipboard (no paste simulation).
    pub fn copy_item(&mut self, id: i64, copy_as_plain_text: bool) {
        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("copy_item: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("copy_item: db error for {id}: {e}");
                return;
            }
        };
        self.touch_item_usage(&item);
        self.write_item_to_clipboard_internal(&item, copy_as_plain_text);
    }

    /// Paste a single item: write to clipboard, restore focus, simulate Ctrl+V.
    pub fn paste_item(&mut self, id: i64, copy_as_plain_text: bool) {
        use crate::core::types::ContentType;
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("paste_item: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("paste_item: db error for {id}: {e}");
                return;
            }
        };

        self.touch_item_usage(&item);
        let is_file = item.content_type == ContentType::File
            || (item.content_type == ContentType::Image && !item.image_path.is_empty());
        let expected = item.full_text.clone();
        self.write_item_to_clipboard_internal(&item, copy_as_plain_text);

        if !expected.is_empty() && !is_file {
            crate::services::clipboard_ops::verify_clipboard_content(&expected, 200);
        }
        if is_file {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        restore_paste_target();
        let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
        paste_after_delay(shortcuts);
    }

    /// Paste an image as bitmap data for apps that do not accept file references.
    pub fn paste_image_as_bitmap(&mut self, id: i64) {
        use crate::core::types::ContentType;
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("paste_image_as_bitmap: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("paste_image_as_bitmap: db error for {id}: {e}");
                return;
            }
        };

        if item.content_type != ContentType::Image || item.image_path.is_empty() {
            return;
        }

        self.touch_item_usage(&item);
        self.bitmap_paste_finished.store(false, Ordering::SeqCst);
        self.show_toast(I18nKey::ToastPreparingBitmapImage.text());
        let image_path = item.image_path.clone();
        let item_id = item.id;
        let batch_pasting = self.batch_pasting.clone();
        let skip_next = self.skip_next.clone();
        let bitmap_paste_finished = self.bitmap_paste_finished.clone();
        let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());

        std::thread::spawn(move || {
            let Some(img_data) = crate::services::clipboard_ops::load_image_bitmap(&image_path)
            else {
                bitmap_paste_finished.store(true, Ordering::SeqCst);
                return;
            };

            let wrote = Self::with_internal_clipboard_write(&batch_pasting, &skip_next, || {
                crate::services::clipboard_ops::write_bitmap_image_to_clipboard(img_data)
            });

            if !wrote {
                log::warn!("paste_image_as_bitmap: failed to write image for item {item_id}");
                bitmap_paste_finished.store(true, Ordering::SeqCst);
                return;
            }

            if !crate::services::clipboard_ops::verify_clipboard_image(500) {
                log::warn!("paste_image_as_bitmap: image verification failed for item {item_id}");
                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            bitmap_paste_finished.store(true, Ordering::SeqCst);
            restore_paste_target();
            paste_after_delay(shortcuts);
        });
    }

    /// Paste a single item as plain text: write plain text to clipboard, restore focus, simulate Ctrl+V.
    pub fn paste_item_plain(&mut self, id: i64) {
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("paste_item_plain: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("paste_item_plain: db error for {id}: {e}");
                return;
            }
        };

        self.touch_item_usage(&item);
        let is_file_or_image = item.content_type == ContentType::File
            || (item.content_type == ContentType::Image && !item.image_path.is_empty());
        let expected = if item.content_type == ContentType::Image && !item.image_path.is_empty() {
            item.image_path.clone()
        } else if item.content_type == ContentType::File && !item.file_data.is_empty() {
            let file_data = FileData::from_json(&item.file_data);
            file_data
                .files
                .iter()
                .map(|f| f.path.clone())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            item.full_text.clone()
        };
        self.write_item_as_plain_text_to_clipboard_internal(&item);

        if !expected.is_empty() {
            crate::services::clipboard_ops::verify_clipboard_content(&expected, 200);
        }
        if is_file_or_image {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        restore_paste_target();
        let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
        paste_after_delay(shortcuts);
    }

    /// Convert a color item from HEX to RGB and paste.
    pub fn paste_as_rgb(&mut self, id: i64) {
        use crate::core::color::detect_color;
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            _ => {
                log::warn!("paste_as_rgb: item {id} not found");
                return;
            }
        };

        if let Some(color) = detect_color(&item.full_text) {
            self.touch_item_usage(&item);
            let rgb_text = color.to_rgb();
            self.write_text_to_clipboard_internal(&rgb_text);
            crate::services::clipboard_ops::verify_clipboard_content(&rgb_text, 200);
            restore_paste_target();
            let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
            paste_after_delay(shortcuts);
        }
    }

    /// Convert a color item from RGB to HEX and paste.
    pub fn paste_as_hex(&mut self, id: i64) {
        use crate::core::color::detect_color;
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            _ => {
                log::warn!("paste_as_hex: item {id} not found");
                return;
            }
        };

        if let Some(color) = detect_color(&item.full_text) {
            self.touch_item_usage(&item);
            let hex_text = color.to_css_hex();
            self.write_text_to_clipboard_internal(&hex_text);
            crate::services::clipboard_ops::verify_clipboard_content(&hex_text, 200);
            restore_paste_target();
            let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
            paste_after_delay(shortcuts);
        }
    }

    /// Batch paste multiple items sequentially.
    pub fn batch_paste(&mut self, ids: &[i64], copy_as_plain_text: bool) {
        use crate::core::types::ContentType;
        use crate::platform::paste::{paste_after_delay, paste_sync, restore_paste_target};

        // --- Suppress clipboard recording during batch paste to prevent ---
        // --- intermediate writes (newline separators) from being captured. ---
        self.batch_pasting
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let items: Vec<crate::core::types::ClipboardItem> = ids
            .iter()
            .filter_map(|&id| self.db.get_by_id(id).ok().flatten())
            .collect();

        // Usage-time update: one transaction + one in-memory reorder for the
        // whole batch (replaces the per-item touch + full list reload).
        self.touch_items_usage(&items);

        let n = items.len();
        for (i, item) in items.iter().enumerate() {
            // --- Newline separator between items (not before first) ---
            if i > 0 {
                crate::services::clipboard_ops::write_text_to_clipboard("\n");
                std::thread::sleep(std::time::Duration::from_millis(20));
                restore_paste_target();
                let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
                paste_sync(shortcuts);
                std::thread::sleep(std::time::Duration::from_millis(60));
            }

            let expected = item.full_text.clone();
            crate::services::clipboard_ops::write_item_to_clipboard(item, copy_as_plain_text);

            // --- Verify clipboard before pasting ---
            if item.content_type == ContentType::Image || item.content_type == ContentType::File {
                if !crate::services::clipboard_ops::verify_clipboard_files(300) {
                    log::warn!("batch_paste: file verification failed for item {}", item.id);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            } else if item.content_type != ContentType::File {
                if !crate::services::clipboard_ops::verify_clipboard_content(&expected, 300) {
                    log::warn!(
                        "batch_paste: text verification timed out for item {}",
                        item.id
                    );
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }

            restore_paste_target();

            if i < n - 1 {
                let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
                paste_sync(shortcuts);
                std::thread::sleep(std::time::Duration::from_millis(100));
            } else {
                let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
                paste_after_delay(shortcuts);
            }
        }
        // --- Restore clipboard recording — batch paste is complete. ---
        self.skip_next
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.batch_pasting
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Merge selected text items into one new entry (seamless concatenation, no separator).
    /// Returns the new item's ID so the UI can scroll to it.
    ///
    /// When all selected items are rich text with HTML content, the merged entry
    /// preserves rich formatting by concatenating the HTML body contents.
    /// Otherwise it falls back to plain-text concatenation of `full_text`.
    pub fn merge_selected_items(&mut self) -> Option<i64> {
        if self.selected_ids.len() < 2 {
            return None;
        }

        // Preserve the explicit selection order. The list is usually sorted by
        // newest first, which can differ from the user's merge order.
        let items: Vec<&ClipboardItem> = self
            .selected_ids
            .iter()
            .filter_map(|id| self.items.iter().find(|item| item.id == *id))
            .collect();

        if items.len() < 2 {
            return None;
        }
        if !items.iter().all(|item| item_can_be_merged(item)) {
            log::warn!("merge_selected_items: selected items include non-text or empty content");
            return None;
        }

        let now = chrono::Utc::now();

        // ── Check whether all items are rich text with HTML ──
        let all_rich = items.iter().all(|item| {
            item.content_type == ContentType::RichText
                && !item.rich_data.is_empty()
                && RichData::from_json(&item.rich_data).html.is_some()
        });

        let (content_type, merged_text, rich_data, hash) = if all_rich {
            // ── Rich text path: merge HTML body contents ──
            let bodies: Vec<String> = items
                .iter()
                .map(|item| {
                    let rich = RichData::from_json(&item.rich_data);
                    let html = rich.html.as_deref().unwrap_or("");
                    extract_html_body(html)
                })
                .collect();
            let merged_body = bodies.concat();
            let merged_html =
                format!("<!DOCTYPE html>\n<html>\n<head><meta charset=\"utf-8\"></head>\n<body>\n{merged_body}\n</body>\n</html>");
            let merged_plain = item_texts_for_merge(&items);

            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&"merge", &mut hasher);
            std::hash::Hash::hash(&now.timestamp_nanos_opt(), &mut hasher);
            std::hash::Hash::hash(&self.selected_ids, &mut hasher);
            std::hash::Hash::hash(&merged_html, &mut hasher);
            let hash = std::hash::Hasher::finish(&hasher);

            let rd = RichData {
                html: Some(merged_html),
                ..Default::default()
            };

            (ContentType::RichText, merged_plain, rd.to_json(), hash)
        } else {
            // ── Plain-text path: concatenate full_text ──
            let merged = item_texts_for_merge(&items);

            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&"merge", &mut hasher);
            std::hash::Hash::hash(&now.timestamp_nanos_opt(), &mut hasher);
            std::hash::Hash::hash(&self.selected_ids, &mut hasher);
            std::hash::Hash::hash(&merged, &mut hasher);
            let hash = std::hash::Hasher::finish(&hasher);

            (ContentType::PlainText, merged, String::new(), hash)
        };

        let text_size = merged_text.chars().count() as i64;
        let item = ClipboardItem {
            id: 0,
            content_type,
            meta_type: String::new(),
            full_text: merged_text,
            content_hash: hash,
            created_at: now,
            updated_at: now,
            image_path: String::new(),
            image_width: 0,
            image_height: 0,
            rich_data,
            file_data: String::new(),
            is_favorite: false,
            note: String::new(),
            source_app_name: String::new(),
            source_app_icon: String::new(),
            size: text_size,
            tags: vec![],
            custom_hotkey: String::new(),
            custom_hotkey_format: String::new(),
            existence_observed_at: String::new(),
        };

        // Write to DB.
        if let Err(e) = self.db.upsert(&item) {
            log::error!("merge_selected_items: upsert failed: {e}");
            return None;
        }

        // Retrieve the auto-generated ID.
        let new_id = match self.db.get_by_hash(hash) {
            Ok(Some(item)) => item.id,
            other => {
                log::error!("merge_selected_items: get_by_hash returned {other:?}");
                return None;
            }
        };

        let merged_count = items.len();

        // Mark sync dirty and refresh the in-memory list.
        self.sync_dirty
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.reload_items();

        // Clear multi-selection, select the new item.
        self.select_single(new_id);

        log::info!(
            "merge_selected_items: merged {merged_count} items → id={new_id} ({text_size} chars)"
        );

        Some(new_id)
    }

    pub fn can_merge_selected_items(&self) -> bool {
        if self.selected_ids.len() < 2 {
            return false;
        }

        let mut count = 0;
        for item in self
            .selected_ids
            .iter()
            .filter_map(|id| self.items.iter().find(|item| item.id == *id))
        {
            if !item_can_be_merged(item) {
                return false;
            }
            count += 1;
        }

        count >= 2
    }

    /// Update the note field for a clipboard item.
    /// Writes to DB (includes updated_at) and syncs the in-memory items list.
    pub fn update_note(&mut self, id: i64, note: &str) {
        let mark_dirty = self
            .items
            .iter()
            .find(|it| it.id == id)
            .is_some_and(|item| self.should_mark_sync_dirty(item));
        match self.db.update_note(id, note) {
            Ok(_) => {
                if mark_dirty {
                    self.sync_dirty.store(true, Ordering::SeqCst);
                }
                if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                    item.note = note.to_string();
                    item.updated_at = chrono::Utc::now();
                }
            }
            Err(e) => log::error!("update_note({id}): {e}"),
        }
    }

    /// Set or update the custom hotkey for a clipboard item.
    pub fn update_item_hotkey(&mut self, id: i64, hotkey: &str, format: &str) {
        match self.db.set_item_hotkey(id, hotkey, format) {
            Ok(_) => {
                if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                    item.custom_hotkey = hotkey.to_string();
                    item.custom_hotkey_format = format.to_string();
                }
                self.refresh_titlebar_filter_availability();
            }
            Err(e) => log::error!("update_item_hotkey({id}): {e}"),
        }
    }

    /// Clear the custom hotkey for a clipboard item.
    pub fn clear_item_hotkey(&mut self, id: i64) {
        match self.db.clear_item_hotkey(id) {
            Ok(_) => {
                if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                    item.custom_hotkey.clear();
                    item.custom_hotkey_format.clear();
                }
                self.refresh_titlebar_filter_availability();
            }
            Err(e) => log::error!("clear_item_hotkey({id}): {e}"),
        }
    }

    /// Get the paste format for a specific item's custom hotkey.
    pub fn get_item_hotkey_format(&self, id: i64) -> crate::core::types::HotkeyPasteFormat {
        self.items
            .iter()
            .find(|i| i.id == id)
            .and_then(|item| {
                if item.custom_hotkey_format.is_empty() {
                    None
                } else {
                    serde_json::from_str(&item.custom_hotkey_format).ok().or(
                        match item.custom_hotkey_format.as_str() {
                            "Default" => Some(crate::core::types::HotkeyPasteFormat::Default),
                            "PlainText" => Some(crate::core::types::HotkeyPasteFormat::PlainText),
                            "ImageBitmap" => {
                                Some(crate::core::types::HotkeyPasteFormat::ImageBitmap)
                            }
                            "ImagePath" => Some(crate::core::types::HotkeyPasteFormat::ImagePath),
                            "OcrText" => Some(crate::core::types::HotkeyPasteFormat::OcrText),
                            "FilePath" => Some(crate::core::types::HotkeyPasteFormat::FilePath),
                            "Rgb" => Some(crate::core::types::HotkeyPasteFormat::Rgb),
                            "Hex" => Some(crate::core::types::HotkeyPasteFormat::Hex),
                            _ => None,
                        },
                    )
                }
            })
            .unwrap_or_default()
    }

    /// Return the id for a latest-item hotkey slot, independent of active UI filters.
    pub fn latest_hotkey_item_id(&self, slot: usize) -> Option<i64> {
        let filters = crate::core::filters::ClipboardFilters::default();
        match self
            .db
            .load_filtered(&filters, slot.saturating_add(1), self.order_by())
        {
            Ok(items) => items.get(slot).map(|item| item.id),
            Err(e) => {
                log::error!("latest_hotkey_item_id({slot}): {e}");
                None
            }
        }
    }

    /// Toggle favorite status for a single item.
    ///
    /// # Tombstones (sync)
    /// - Favorited → unfavorited: records `unfavorited_items` tombstone
    /// - Unfavorited → favorited: removes existing `unfavorited_items` tombstone
    /// - Sets `sync_dirty = true`
    ///
    /// # Incremental update
    /// Updates `item.is_favorite` and `item.updated_at` in `self.items` directly,
    /// unless the favorites filter is active or favorites-first search reordering
    /// applies (both need a full reload for accurate results).
    pub fn toggle_favorite(&mut self, id: i64) {
        let needs_full_refresh = self.filters.is_favorites_active()
            || (self.settings.search_favorites_first && self.filters.has_keyword());

        // Read current state before toggling (needed for tombstone direction)
        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("toggle_favorite({id}): item not found");
                return;
            }
            Err(e) => {
                log::error!("toggle_favorite({id}): {e}");
                return;
            }
        };
        let was_fav = item.is_favorite;
        let hash = item.content_hash;

        if let Err(e) = self.db.toggle_favorite(id) {
            log::error!("toggle_favorite({id}): {e}");
            return;
        }

        // --- Tombstone management ---
        if was_fav {
            // --- Was favorited, now unfavorited — record tombstone ---
            let now = chrono::Utc::now().to_rfc3339();
            let device = crate::services::backends::local_folder::hostname();
            if let Err(e) = self.db.record_unfavorite(hash, &now, &device) {
                log::error!("record_unfavorite({hash}): {e}");
            }
        } else {
            // --- Was unfavorited, now favorited — remove tombstone ---
            if let Err(e) = self.db.remove_unfavorite(hash) {
                log::error!("remove_unfavorite({hash}): {e}");
            }
        }

        self.sync_dirty.store(true, Ordering::SeqCst);

        if needs_full_refresh {
            self.reload_items();
            self.clear_selection();
        } else {
            // --- Incremental update: flip is_favorite + bump updated_at ---
            if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                item.is_favorite = !item.is_favorite;
                item.updated_at = chrono::Utc::now();
            }
            self.refresh_titlebar_filter_availability();
        }
    }

    /// Delete a single item and record deletion tombstone for sync.
    ///
    /// # Tombstones (sync)
    /// - Records `deleted_items` tombstone with content_hash, timestamp, device_name
    /// - Sets `sync_dirty = true`
    ///
    /// # Side effects
    /// - Removes item from `self.items`
    /// - Removes id from `self.selected_ids`
    pub fn delete_item(&mut self, id: i64) {
        // Read item first to get content_hash for tombstone
        if let Ok(Some(item)) = self.db.get_by_id(id) {
            let hash = item.content_hash;
            let now = chrono::Utc::now().to_rfc3339();
            let device = crate::services::backends::local_folder::hostname();

            if let Err(e) = self.db.delete_item(id) {
                log::error!("delete_item({id}): {e}");
                return;
            }
            self.clear_deleted_file_transfer_association(&item);

            // Record deletion tombstone only for items in sync scope.
            // Image/file items are never synced; unfavorited items in
            // favorites-only mode have already left the sync scope via
            // the unfavorite tombstone.
            if self.should_mark_sync_dirty(&item) {
                if let Err(e) = self.db.record_item_deletion(hash, &now, &device) {
                    log::error!("record_item_deletion({hash}): {e}");
                }
                self.sync_dirty.store(true, Ordering::SeqCst);
            }
            // If this item had a custom hotkey, schedule it for unregistration.
            if !item.custom_hotkey.is_empty() {
                self.pending_hotkey_unregister.push(id);
            }
        } else {
            log::warn!("delete_item({id}): item not found");
            return;
        }

        // --- Remove from in-memory items and selection ---
        self.items.retain(|it| it.id != id);
        self.selected_ids.retain(|&sid| sid != id);
        self.refresh_titlebar_filter_availability();
    }

    /// Batch toggle favorite on all selected items.
    /// Loops selected_ids, applies the same toggle + tombstone logic per item.
    pub fn batch_toggle_favorite(&mut self) {
        let needs_full_refresh = self.filters.is_favorites_active()
            || (self.settings.search_favorites_first && self.filters.has_keyword());
        let now = chrono::Utc::now().to_rfc3339();
        let device = crate::services::backends::local_folder::hostname();
        let updated_at = chrono::Utc::now();

        let ids: Vec<i64> = self.selected_ids.clone();
        for &id in &ids {
            let item = match self.db.get_by_id(id) {
                Ok(Some(item)) => item,
                Ok(None) => {
                    log::warn!("batch toggle_favorite({id}): item not found");
                    continue;
                }
                Err(e) => {
                    log::error!("batch get_by_id({id}): {e}");
                    continue;
                }
            };
            let was_fav = item.is_favorite;
            let hash = item.content_hash;

            if let Err(e) = self.db.toggle_favorite(id) {
                log::error!("batch toggle_favorite({id}): {e}");
                continue;
            }

            if was_fav {
                if let Err(e) = self.db.record_unfavorite(hash, &now, &device) {
                    log::error!("batch record_unfavorite({hash}): {e}");
                }
            } else if let Err(e) = self.db.remove_unfavorite(hash) {
                log::error!("batch remove_unfavorite({hash}): {e}");
            }
        }

        if needs_full_refresh {
            self.reload_items();
            self.clear_selection();
        } else {
            // Incremental update: flip is_favorite + bump updated_at for each
            for id in &ids {
                if let Some(item) = self.items.iter_mut().find(|it| &it.id == id) {
                    item.is_favorite = !item.is_favorite;
                    item.updated_at = updated_at;
                }
            }
            self.refresh_titlebar_filter_availability();
        }

        self.sync_dirty.store(true, Ordering::SeqCst);
    }

    /// Batch delete all selected items.
    /// Records deletion tombstones only for items in sync scope.
    pub fn batch_delete(&mut self) {
        let now = chrono::Utc::now().to_rfc3339();
        let device = crate::services::backends::local_folder::hostname();

        // --- Collect hashes only for items in sync scope. ---
        let mut hashes: Vec<u64> = Vec::with_capacity(self.selected_ids.len());
        let mut has_sync_item = false;
        let selected_ids = self.selected_ids.clone();
        for id in selected_ids {
            if let Ok(Some(item)) = self.db.get_by_id(id) {
                let in_sync = self.should_mark_sync_dirty(&item);
                if in_sync {
                    has_sync_item = true;
                }
                match self.db.delete_item(id) {
                    Ok(_) => {
                        self.clear_deleted_file_transfer_association(&item);
                        if in_sync {
                            hashes.push(item.content_hash);
                        }
                        if !item.custom_hotkey.is_empty() {
                            self.pending_hotkey_unregister.push(id);
                        }
                    }
                    Err(e) => log::error!("batch delete_item({id}): {e}"),
                }
            }
        }

        // Record tombstones for sync
        for h in &hashes {
            if let Err(e) = self.db.record_item_deletion(*h, &now, &device) {
                log::error!("batch record_item_deletion({h}): {e}");
            }
        }

        if has_sync_item {
            self.sync_dirty.store(true, Ordering::SeqCst);
        }

        // --- Remove from in-memory items ---
        let ids: Vec<i64> = self.selected_ids.drain(..).collect();
        self.items.retain(|it| !ids.contains(&it.id));
        self.refresh_titlebar_filter_availability();
    }

    /// A transfer entry is considered local only while a local clipboard row
    /// owns its path. Removing that row must not delete the file or the remote
    /// object, but it does remove the hidden transfer backing association.
    fn clear_deleted_file_transfer_association(&mut self, deleted_item: &ClipboardItem) {
        if deleted_item.content_type != ContentType::File {
            return;
        }
        let deleted_paths = FileData::from_json(&deleted_item.file_data)
            .files
            .into_iter()
            .map(|file| file.path)
            .collect();
        self.clear_deleted_file_transfer_associations(deleted_paths);
    }

    /// Remove hidden local transfer associations for file paths deleted by a
    /// background maintenance task. Source files and remote objects are kept.
    pub fn clear_deleted_file_transfer_associations(&mut self, deleted_paths: Vec<String>) {
        let deleted_paths = deleted_paths
            .into_iter()
            .map(|path| transfer_path_key(&path))
            .collect::<HashSet<_>>();
        if deleted_paths.is_empty() {
            return;
        }

        let remaining_paths = self
            .db
            .get_original_file_items()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|item| FileData::from_json(&item.file_data).files)
            .map(|file| transfer_path_key(&file.path))
            .collect::<HashSet<_>>();
        let unreferenced_paths = deleted_paths
            .difference(&remaining_paths)
            .cloned()
            .collect::<HashSet<_>>();
        if unreferenced_paths.is_empty() {
            return;
        }

        let hashes = self
            .db
            .get_transfer_items()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| {
                let file_data = FileData::from_json(&item.file_data);
                file_data
                    .files
                    .iter()
                    .any(|file| unreferenced_paths.contains(&transfer_path_key(&file.path)))
                    .then_some(file_data.remote_hash)
            })
            .filter(|hash| !hash.is_empty())
            .collect::<HashSet<_>>();
        for hash in hashes {
            if let Err(error) = self.db.delete_transfer_by_hash(&hash) {
                log::warn!("delete local transfer association {hash}: {error}");
                continue;
            }
            if let Some(entry) = self
                .transfer_entries
                .iter_mut()
                .find(|entry| entry.entry.hash == hash)
            {
                entry.is_local = false;
                entry.local_path = None;
            }
        }
    }
}

fn transfer_path_key(path: &str) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path))
}

fn open_system_target(target: &str) {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;

        let operation: Vec<u16> = "open\0".encode_utf16().collect();
        let target_utf16: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let ret = ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                target_utf16.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOW,
            );
            // ShellExecuteW returns a value > 32 on success; <= 32 is an error.
            if (ret as isize) <= 32 {
                log::error!(
                    "ShellExecuteW failed for '{}': error code {}",
                    target,
                    ret as isize,
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(target).spawn();
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(target).spawn();
    }
}

fn reveal_file_location(path: &str) {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;

        let operation: Vec<u16> = "open\0".encode_utf16().collect();
        let explorer: Vec<u16> = "explorer\0".encode_utf16().collect();
        let arg = format!("/select,\"{}\"", path);
        let arg_utf16: Vec<u16> = arg.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                explorer.as_ptr(),
                arg_utf16.as_ptr(),
                std::ptr::null(),
                SW_SHOW,
            );
        }
    }

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
        }
    }
}

/// Extract body content from an HTML string for merging.
///
/// If the HTML has `<body>…</body>` tags, returns the inner content;
/// otherwise returns the input as-is.
fn extract_html_body(html: &str) -> String {
    let lower = html.to_lowercase();
    if let Some(body_start) = lower.find("<body") {
        if let Some(tag_end) = html[body_start..].find('>') {
            let content_start = body_start + tag_end + 1;
            if let Some(body_end) = lower[content_start..].find("</body>") {
                return html[content_start..content_start + body_end]
                    .trim()
                    .to_string();
            }
        }
    }
    html.trim().to_string()
}

/// Concatenate the `full_text` of all items (no separator).
fn item_texts_for_merge(items: &[&ClipboardItem]) -> String {
    items.iter().map(|item| item.full_text.as_str()).collect()
}

// --- ── Transfer station methods ── ---

impl AppState {
    pub fn transfer_available(&self) -> bool {
        self.settings.transfer_station_enabled
            && self.settings.sync_backends.iter().any(|backend| {
                backend.enabled
                    && (self.settings.transfer_backend_id.is_empty()
                        || backend.id == self.settings.transfer_backend_id)
            })
    }

    /// Toggle the transfer station view. Returns `false` when the station
    /// could not be activated (no enabled matching backend); the caller must
    /// not switch pages or record a return view in that case.
    pub fn toggle_transfer_filter(&mut self) -> bool {
        if !self.transfer_filter_active && !self.transfer_available() {
            self.toast_message = Some(I18nKey::TransferNoBackend.text().into());
            return false;
        }
        self.transfer_filter_active = !self.transfer_filter_active;
        if self.transfer_filter_active {
            // Only the explicit refresh triggered by entering the transfer view
            // shows the global loading indicator. Periodic polling stays silent.
            self.transfer_refreshing = true;
            self.queue_transfer_command(
                crate::services::transfer_station::TransferCommand::Refresh,
            );
        } else {
            self.transfer_refreshing = false;
            self.pending_transfer_commands.retain(|command| {
                !matches!(
                    command,
                    crate::services::transfer_station::TransferCommand::Refresh
                )
            });
            // Re-apply the current search term to the database-backed list.
            self.reload_items();
        }
        true
    }

    pub fn queue_daily_transfer_cleanup(&mut self) {
        if !self.pending_transfer_commands.iter().any(|command| {
            matches!(
                command,
                crate::services::transfer_station::TransferCommand::Cleanup
            )
        }) {
            self.queue_transfer_command(
                crate::services::transfer_station::TransferCommand::Cleanup,
            );
        }
    }

    fn queue_transfer_command(
        &mut self,
        command: crate::services::transfer_station::TransferCommand,
    ) {
        self.pending_transfer_commands.push_back(command);
        self.transfer_busy = true;
    }

    pub fn upload_to_transfer_station(&mut self, item_id: i64) {
        if !self.transfer_available() {
            self.toast_message = Some(I18nKey::TransferNoBackend.text().into());
            return;
        }
        let item = match self.db.get_by_id(item_id).ok().flatten() {
            Some(item) if item.content_type == ContentType::File => item,
            _ => {
                self.toast_message = Some(I18nKey::TransferNotFile.text().into());
                return;
            }
        };
        let file_data = FileData::from_json(&item.file_data);
        if file_data.is_transfer() || !file_data.remote_hash.is_empty() {
            self.toast_message = Some(I18nKey::TransferAlreadyUploaded.text().into());
            return;
        }
        if file_data.files.len() != 1 {
            self.toast_message = Some(I18nKey::TransferSingleFileOnly.text().into());
            return;
        }
        let file = &file_data.files[0];
        if file.is_dir || !std::path::Path::new(&file.path).is_file() {
            self.toast_message = Some(I18nKey::TransferInvalidPath.text().into());
            return;
        }
        if !self.pending_transfer_uploads.insert(file.path.clone()) {
            return;
        }
        self.queue_transfer_command(crate::services::transfer_station::TransferCommand::Upload {
            source_path: file.path.clone(),
            file_name: file.name.clone(),
        });
    }

    pub fn download_transfer_entry(&mut self, hash: &str) {
        if self.pending_transfer_downloads.contains(hash) {
            return;
        }
        let Some(entry) = self
            .transfer_entries
            .iter()
            .find(|resolved| resolved.entry.hash == hash)
            .map(|resolved| resolved.entry.clone())
        else {
            self.toast_message = Some(I18nKey::TransferEntryExpired.text().into());
            return;
        };
        self.pending_transfer_downloads.insert(hash.to_string());
        self.queue_transfer_command(
            crate::services::transfer_station::TransferCommand::Download { entry },
        );
    }

    pub fn delete_transfer_entries(&mut self, hashes: &[String]) {
        let entries = hashes
            .iter()
            .filter_map(|hash| {
                self.transfer_entries
                    .iter()
                    .find(|resolved| resolved.entry.hash == *hash)
                    .map(|resolved| resolved.entry.clone())
            })
            .collect::<Vec<_>>();
        for entry in entries {
            self.queue_transfer_command(
                crate::services::transfer_station::TransferCommand::Delete { entry },
            );
        }
    }

    pub fn delete_transfer_entry(&mut self, hash: &str) {
        let Some(entry) = self
            .transfer_entries
            .iter()
            .find(|resolved| resolved.entry.hash == hash)
            .map(|resolved| resolved.entry.clone())
        else {
            self.toast_message = Some(I18nKey::TransferEntryExpired.text().into());
            return;
        };
        self.queue_transfer_command(crate::services::transfer_station::TransferCommand::Delete {
            entry,
        });
    }

    /// Queue a pin/unpin command for a transfer entry. Duplicate clicks for the
    /// same hash are ignored while the first command is still in flight; the
    /// remote manifest remains the single source of truth for the final state.
    pub fn set_transfer_entry_pinned(&mut self, hash: &str, pinned: bool) {
        if self.pending_transfer_pin_updates.contains(hash) {
            return;
        }
        let Some(entry) = self
            .transfer_entries
            .iter()
            .find(|resolved| resolved.entry.hash == hash)
            .map(|resolved| resolved.entry.clone())
        else {
            self.toast_message = Some(I18nKey::TransferEntryExpired.text().into());
            return;
        };
        self.pending_transfer_pin_updates.insert(hash.to_string());
        self.queue_transfer_command(
            crate::services::transfer_station::TransferCommand::SetPinned { entry, pinned },
        );
    }

    pub fn open_transfer_location(&self, path: &str) {
        if !std::path::Path::new(path).is_file() {
            return;
        }
        let path = path.to_string();
        std::thread::spawn(move || reveal_file_location(&path));
    }

    /// Get visible items based on current filter mode.
    /// In transfer mode, returns converted manifest entries (keyword-filtered
    /// on the remote file name — never touching the DB). Otherwise returns DB items.
    pub fn visible_items(&self) -> Vec<ClipboardItem> {
        if self.transfer_filter_active {
            let terms = self.filters.keyword_terms();
            let retention_days = self.settings.transfer_retention_days;
            self.transfer_entries
                .iter()
                .filter(|re| {
                    terms.is_empty()
                        || crate::core::search::text_matches_all_terms(&re.entry.name, &terms)
                })
                .map(|re| {
                    let uploaded_at: chrono::DateTime<chrono::Utc> =
                        chrono::DateTime::parse_from_rfc3339(&re.entry.uploaded_at)
                            .map(|dt| dt.with_timezone(&chrono::Utc))
                            .unwrap_or_else(|_| chrono::Utc::now());
                    // Determine status tags based on is_local
                    let mut status_tags =
                        if self.pending_transfer_downloads.contains(&re.entry.hash) {
                            vec![TagInfo {
                                id: -3,
                                uid: TRANSFER_STATUS_DOWNLOADING_UID.into(),
                                name: I18nKey::TransferDownloading.text().to_string(),
                                color: TRANSFER_BLUE.into(),
                                updated_at: String::new(),
                            }]
                        } else if re.is_local {
                            vec![TagInfo {
                                id: -1,
                                uid: TRANSFER_STATUS_LOCAL_UID.into(),
                                name: I18nKey::TransferLocal.text().to_string(),
                                color: "#22C55E".to_string(),
                                updated_at: String::new(),
                            }]
                        } else {
                            vec![TagInfo {
                                id: -2,
                                uid: TRANSFER_STATUS_CLOUD_UID.into(),
                                name: I18nKey::TransferCloud.text().to_string(),
                                color: TRANSFER_BLUE.into(),
                                updated_at: String::new(),
                            }]
                        };
                    // Pinned marker + effective expiration metadata live only on
                    // these in-memory virtual items; the remote manifest stays
                    // the single source of truth (no DB persistence).
                    if re.entry.pinned {
                        status_tags.push(TagInfo {
                            id: -4,
                            uid: TRANSFER_STATUS_PINNED_UID.into(),
                            name: I18nKey::TransferKeepForever.text().to_string(),
                            color: TRANSFER_BLUE.into(),
                            updated_at: String::new(),
                        });
                    }
                    status_tags.push(TagInfo {
                        id: -5,
                        uid: TRANSFER_STATUS_RETENTION_UID.into(),
                        name: String::new(),
                        color: TRANSFER_BLUE.into(),
                        updated_at: crate::core::transfer_types::effective_expiration(
                            &re.entry,
                            retention_days,
                        )
                        .map(|expires| expires.to_rfc3339())
                        .unwrap_or_default(),
                    });
                    ClipboardItem {
                        id: transfer_item_id(&re.entry.hash),
                        content_type: ContentType::File,
                        full_text: re.entry.name.clone(),
                        content_hash: 0,
                        created_at: uploaded_at,
                        updated_at: uploaded_at,
                        image_path: String::new(),
                        image_width: 0,
                        image_height: 0,
                        rich_data: String::new(),
                        file_data: FileData {
                            files: vec![crate::core::types::FileInfo {
                                name: re.entry.name.clone(),
                                path: re.local_path.clone().unwrap_or_default(),
                                is_dir: false,
                            }],
                            transfer: true,
                            remote_hash: re.entry.hash.clone(),
                        }
                        .to_json(),
                        is_favorite: false,
                        note: String::new(),
                        source_app_name: String::new(),
                        source_app_icon: String::new(),
                        size: re.entry.size as i64,
                        tags: status_tags,
                        meta_type: "transfer".to_string(),
                        custom_hotkey: String::new(),
                        custom_hotkey_format: String::new(),
                        existence_observed_at: String::new(),
                    }
                })
                .collect()
        } else {
            // Transfer records persist local cache/original paths in the shared
            // clipboard table, but they are internal backing records rather than
            // ordinary clipboard history. Render them only through the dedicated
            // transfer view above, otherwise an uploaded file appears twice.
            self.items
                .iter()
                .filter(|item| {
                    item.content_type != ContentType::File
                        || !FileData::from_json(&item.file_data).is_transfer()
                })
                .cloned()
                .collect()
        }
    }
}

fn transfer_item_id(hash: &str) -> i64 {
    let value = u64::from_str_radix(hash.get(..15).unwrap_or_default(), 16).unwrap_or(1);
    -(value.max(1) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db::Database;
    use crate::core::filters::ClipboardFilters;
    use crate::core::settings::AppSettings;
    use crate::core::types::{ClipboardItem, ContentType, RichData};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    fn make_item(
        id: i64,
        content_type: ContentType,
        is_favorite: bool,
        full_text: &str,
    ) -> ClipboardItem {
        ClipboardItem {
            id,
            content_type,
            full_text: full_text.to_string(),
            content_hash: 0x100 + id as u64,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            image_path: String::new(),
            image_width: 0,
            image_height: 0,
            rich_data: String::new(),
            file_data: String::new(),
            is_favorite,
            note: String::new(),
            source_app_name: String::new(),
            source_app_icon: String::new(),
            size: full_text.len() as i64,
            tags: Vec::new(),
            meta_type: String::new(),
            custom_hotkey: String::new(),
            custom_hotkey_format: String::new(),
            existence_observed_at: String::new(),
        }
    }

    fn backend_config(id: &str, enabled: bool) -> crate::core::settings::BackendConfig {
        crate::core::settings::BackendConfig {
            id: id.to_string(),
            enabled,
            backend_type: "local_folder".to_string(),
            name: "test".to_string(),
            folder_path: String::new(),
            device_name: String::new(),
            last_sync_at: String::new(),
            last_item_count: 0,
            last_tag_count: 0,
            sync_interval_secs: None,
            webdav_url: String::new(),
            webdav_root_url: String::new(),
            webdav_path: String::new(),
            webdav_username: String::new(),
            webdav_password: String::new(),
        }
    }

    fn test_state() -> (AppState, Arc<AtomicBool>) {
        let db = Database::open(":memory:").unwrap();
        let dirty = Arc::new(AtomicBool::new(false));
        let settings = AppSettings {
            db_path: ":memory:".to_string(),
            ..Default::default()
        };
        let state = AppState {
            settings,
            db,
            items: Vec::new(),
            pending_images: Vec::new(),
            tags: Vec::new(),
            filters: ClipboardFilters::default(),
            has_hotkey_items: false,
            has_favorite_items: false,
            clearable_history_count: 0,
            clearable_non_favorite_history_count: 0,
            has_transfer_files: false,
            transfer_filter_active: false,
            transfer_entries: Vec::new(),
            pending_transfer_commands: VecDeque::new(),
            pending_transfer_downloads: HashSet::new(),
            pending_transfer_pin_updates: HashSet::new(),
            pending_transfer_uploads: HashSet::new(),
            transfer_busy: false,
            transfer_refreshing: false,
            selected_ids: Vec::new(),
            editing_tag_id: -1,
            editing_tag_name: String::new(),
            editing_tag_color: "#3B82F6".into(),
            editing_item_id: -1,
            editing_item: None,
            batch_pasting: Arc::new(AtomicBool::new(false)),
            skip_next: Arc::new(AtomicBool::new(false)),
            sync_dirty: dirty.clone(),
            bitmap_paste_finished: Arc::new(AtomicBool::new(false)),
            last_usage_touched_ids: Vec::new(),
            usage_sync_requires_full_reload: false,
            pending_hotkey_unregister: Vec::new(),
            toast_message: None,
            toast_is_warning: false,
            foreground_app_name: String::new(),
            foreground_window_title: String::new(),
            foreground_app_icon_base64: String::new(),
            hotkey_recording: false,
            recording_quick_hotkey: false,
            pending_single_hotkey: None,
            sync: SyncState::default(),
            update_available: None,
            update_phase: UpdatePhase::Idle,
        };
        (state, dirty)
    }

    /// Insert a plain-text item with a controlled timestamp offset (seconds in
    /// the past) and return its real database id.
    ///
    /// `upsert` intentionally never writes `is_favorite` (captured items start
    /// as non-favorites), so favorite items are set via `toggle_favorite` and
    /// then re-upserted to restore the controlled `updated_at`.
    fn insert_item_at_age(
        state: &mut AppState,
        id: i64,
        is_favorite: bool,
        full_text: &str,
        age_secs: i64,
    ) -> i64 {
        let mut item = make_item(id, ContentType::PlainText, is_favorite, full_text);
        let ts = chrono::Utc::now() - chrono::Duration::seconds(age_secs);
        item.created_at = ts;
        item.updated_at = ts;
        let hash = item.content_hash;
        state.db.upsert(&item).unwrap();
        let real_id = state.db.get_by_hash(hash).unwrap().unwrap().id;
        if is_favorite {
            state.db.toggle_favorite(real_id).unwrap();
            state.db.upsert(&item).unwrap(); // restore updated_at, keep is_favorite
        }
        real_id
    }

    fn item_texts(state: &AppState) -> Vec<&str> {
        state.items.iter().map(|i| i.full_text.as_str()).collect()
    }

    /// Insert a path-type item; used with `filter_foreign_paths` tests.
    fn insert_path_item_at_age(
        state: &mut AppState,
        id: i64,
        full_path: &str,
        age_secs: i64,
    ) -> i64 {
        let mut item = make_item(id, ContentType::PlainText, false, full_path);
        item.meta_type = "path".to_string();
        let ts = chrono::Utc::now() - chrono::Duration::seconds(age_secs);
        item.created_at = ts;
        item.updated_at = ts;
        let hash = item.content_hash;
        state.db.upsert(&item).unwrap();
        state.db.get_by_hash(hash).unwrap().unwrap().id
    }

    #[test]
    fn keyword_search_skips_non_matching_pages_when_disabled() {
        let (mut state, _dirty) = test_state();
        // The first 128 candidates (one full page) contain no match; the match
        // sits on the next page. An empty filtered page must not end the scan.
        for i in 0..128 {
            insert_item_at_age(&mut state, 1000 + i, false, "filler", 1 + i);
        }
        insert_item_at_age(&mut state, 2000, false, "needle late", 1000);

        state.filters.set_keyword("needle");
        state.reload_items();

        assert_eq!(item_texts(&state), vec!["needle late"]);
    }

    #[test]
    fn keyword_search_skips_non_matching_pages_when_enabled() {
        let (mut state, _dirty) = test_state();
        state.settings.search_favorites_first = true;
        // Same one-page gap; the later favorite and regular matches must both
        // be found and grouped correctly.
        for i in 0..128 {
            insert_item_at_age(&mut state, 1000 + i, false, "filler", 1 + i);
        }
        let fav_id = insert_item_at_age(&mut state, 2000, true, "needle fav late", 1000);
        insert_item_at_age(&mut state, 2001, false, "needle reg late", 1001);

        state.filters.set_keyword("needle");
        state.reload_items();

        assert_eq!(state.items.len(), 2);
        assert_eq!(state.items[0].id, fav_id);
        assert!(state.items[0].is_favorite);
        assert!(!state.items[1].is_favorite);
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn keyword_search_continues_after_fully_filtered_foreign_path_page() {
        let (mut state, _dirty) = test_state();
        state.settings.filter_foreign_paths = true;
        // One full page of foreign paths is filtered out entirely; the scan
        // must continue to the native path match on the next page.
        // The foreign path is chosen per platform: Unix absolute paths are
        // foreign on Windows, Windows-style paths are foreign on macOS.
        // Linux is excluded because its path_is_native() stub accepts all
        // paths, so the path filter can never remove a page there.
        #[cfg(target_os = "windows")]
        let foreign_path = "/usr/share/lib/file.txt";
        #[cfg(target_os = "macos")]
        let foreign_path = "C:\\share\\lib\\file.txt";
        #[cfg(target_os = "windows")]
        let native_path = "C:\\needle\\file.txt";
        #[cfg(target_os = "macos")]
        let native_path = "/Users/needle/file.txt";

        for i in 0..128 {
            insert_path_item_at_age(&mut state, 1000 + i, foreign_path, 1 + i);
        }
        insert_path_item_at_age(&mut state, 2000, native_path, 1000);

        state.filters.set_keyword("needle");
        state.reload_items();

        assert_eq!(item_texts(&state), vec![native_path]);
    }

    #[test]
    fn keyword_search_keeps_time_order_when_favorites_first_disabled() {
        let (mut state, _dirty) = test_state();
        insert_item_at_age(&mut state, 1, false, "needle reg-new", 1);
        insert_item_at_age(&mut state, 2, false, "needle reg-mid", 2);
        insert_item_at_age(&mut state, 3, true, "needle fav-old", 3);

        state.filters.set_keyword("needle");
        state.reload_items();

        assert_eq!(
            item_texts(&state),
            vec!["needle reg-new", "needle reg-mid", "needle fav-old"]
        );
    }

    #[test]
    fn keyword_search_prioritizes_favorites_when_enabled() {
        let (mut state, _dirty) = test_state();
        state.settings.search_favorites_first = true;
        insert_item_at_age(&mut state, 1, false, "needle reg-new", 1);
        insert_item_at_age(&mut state, 2, false, "needle reg-mid", 2);
        insert_item_at_age(&mut state, 3, true, "needle fav-old", 3);

        state.filters.set_keyword("needle");
        state.reload_items();

        assert_eq!(
            item_texts(&state),
            vec!["needle fav-old", "needle reg-new", "needle reg-mid"]
        );
    }

    #[test]
    fn keyword_search_keeps_time_order_within_groups() {
        let (mut state, _dirty) = test_state();
        state.settings.search_favorites_first = true;
        insert_item_at_age(&mut state, 1, true, "needle fav-newer", 1);
        insert_item_at_age(&mut state, 2, false, "needle reg-newer", 2);
        insert_item_at_age(&mut state, 3, true, "needle fav-older", 3);
        insert_item_at_age(&mut state, 4, false, "needle reg-older", 4);

        state.filters.set_keyword("needle");
        state.reload_items();

        assert_eq!(
            item_texts(&state),
            vec![
                "needle fav-newer",
                "needle fav-older",
                "needle reg-newer",
                "needle reg-older"
            ]
        );
    }

    #[test]
    fn keyword_search_groups_respect_sort_by_created() {
        let (mut state, _dirty) = test_state();
        state.settings.search_favorites_first = true;
        state.settings.sort_by_created = true;
        insert_item_at_age(&mut state, 1, false, "needle reg-newer", 1);
        insert_item_at_age(&mut state, 2, true, "needle fav-newer", 2);
        insert_item_at_age(&mut state, 3, false, "needle reg-older", 3);
        insert_item_at_age(&mut state, 4, true, "needle fav-older", 4);

        state.filters.set_keyword("needle");
        state.reload_items();

        assert_eq!(
            item_texts(&state),
            vec![
                "needle fav-newer",
                "needle fav-older",
                "needle reg-newer",
                "needle reg-older"
            ]
        );
    }

    #[test]
    fn favorites_first_does_not_reorder_main_list_without_keyword() {
        let (mut state, _dirty) = test_state();
        state.settings.search_favorites_first = true;
        insert_item_at_age(&mut state, 1, false, "alpha", 1);
        insert_item_at_age(&mut state, 2, true, "beta", 2);
        insert_item_at_age(&mut state, 3, false, "gamma", 3);

        state.reload_items();

        assert_eq!(item_texts(&state), vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn favorites_first_with_favorites_only_filter_matches_existing_behavior() {
        let (mut state, _dirty) = test_state();
        state.settings.search_favorites_first = true;
        insert_item_at_age(&mut state, 1, false, "needle reg", 1);
        insert_item_at_age(&mut state, 2, true, "needle fav-a", 2);
        insert_item_at_age(&mut state, 3, true, "needle fav-b", 3);

        state.filters.set_keyword("needle");
        state.filters.toggle_favorites_only();
        state.reload_items();

        assert_eq!(item_texts(&state), vec!["needle fav-a", "needle fav-b"]);
    }

    #[test]
    fn favorites_beyond_old_stop_point_still_enter_results_when_enabled() {
        let (mut state, _dirty) = test_state();
        state.settings.search_favorites_first = true;
        // result_limit is 200 by default; the first 201 scanned candidates are
        // regular matches, so the old path would stop before ever seeing the
        // favorites. They are still within the 1000-row scan limit.
        for i in 0..201 {
            insert_item_at_age(&mut state, 1000 + i, false, "needle", 10 + i);
        }
        insert_item_at_age(&mut state, 3000, true, "needle fav-old-a", 1000);
        insert_item_at_age(&mut state, 3001, true, "needle fav-old-b", 1001);

        state.filters.set_keyword("needle");
        state.reload_items();

        assert_eq!(state.items.len(), 200);
        assert!(state.items[0].is_favorite);
        assert!(state.items[1].is_favorite);
        assert_eq!(state.items[0].full_text, "needle fav-old-a");
        assert_eq!(state.items[1].full_text, "needle fav-old-b");
        assert!(!state.items[2].is_favorite);
    }

    #[test]
    fn favorites_beyond_stop_point_not_found_when_disabled() {
        let (mut state, _dirty) = test_state();
        for i in 0..201 {
            insert_item_at_age(&mut state, 1000 + i, false, "needle", 10 + i);
        }
        insert_item_at_age(&mut state, 3000, true, "needle fav-old", 1000);

        state.filters.set_keyword("needle");
        state.reload_items();

        assert_eq!(state.items.len(), 200);
        assert!(state.items.iter().all(|i| !i.is_favorite));
    }

    #[test]
    fn favorite_bucket_fill_ends_scan_early_and_caps_result() {
        let (mut state, _dirty) = test_state();
        state.settings.search_favorites_first = true;
        for i in 0..201 {
            insert_item_at_age(&mut state, 1000 + i, true, "needle", 10 + i);
        }
        insert_item_at_age(&mut state, 3000, false, "needle reg", 1000);

        state.filters.set_keyword("needle");
        state.reload_items();

        assert_eq!(state.items.len(), 200);
        assert!(state.items.iter().all(|i| i.is_favorite));
    }

    #[test]
    fn toggling_favorite_during_search_reorders_results() {
        let (mut state, _dirty) = test_state();
        state.settings.search_favorites_first = true;
        insert_item_at_age(&mut state, 1, false, "needle reg", 1);
        let fav_id = insert_item_at_age(&mut state, 2, true, "needle fav", 2);
        let reg_id = insert_item_at_age(&mut state, 3, false, "needle other", 3);

        state.filters.set_keyword("needle");
        state.reload_items();
        assert_eq!(state.items[0].id, fav_id);

        // Full refresh must move the toggled item into the favorite group.
        state.toggle_favorite(reg_id);
        assert_eq!(state.items[0].id, reg_id);
        assert!(state.items[0].is_favorite);
        assert_eq!(state.items[1].id, fav_id);
    }

    #[test]
    fn toggling_favorite_without_keyword_keeps_incremental_update() {
        let (mut state, _dirty) = test_state();
        state.settings.search_favorites_first = true;
        insert_item_at_age(&mut state, 1, false, "alpha", 1);
        let beta_id = insert_item_at_age(&mut state, 2, false, "beta", 2);
        insert_item_at_age(&mut state, 3, true, "gamma", 3);

        state.reload_items();
        assert_eq!(state.items[0].full_text, "alpha");

        // No keyword: incremental path keeps the list order in place.
        state.toggle_favorite(beta_id);
        assert_eq!(state.items[0].full_text, "alpha");
        assert!(state.items[1].is_favorite);
    }

    #[test]
    fn batch_toggling_favorites_during_search_reorders_results() {
        let (mut state, _dirty) = test_state();
        state.settings.search_favorites_first = true;
        let reg_a = insert_item_at_age(&mut state, 1, false, "needle a", 1);
        let reg_b = insert_item_at_age(&mut state, 2, false, "needle b", 2);
        let fav_id = insert_item_at_age(&mut state, 3, true, "needle fav", 3);
        let reg_c = insert_item_at_age(&mut state, 4, false, "needle c", 4);

        state.filters.set_keyword("needle");
        state.reload_items();
        assert_eq!(state.items[0].id, fav_id);

        state.selected_ids = vec![reg_a, reg_b];
        state.batch_toggle_favorite();

        // Both toggled items join the favorite group; the untouched favorite
        // stays there and the non-favorite stays at the end.
        assert_eq!(state.items.len(), 4);
        assert!(state.items[0].is_favorite);
        assert!(state.items[1].is_favorite);
        assert!(state.items[2].is_favorite);
        assert_eq!(state.items[2].id, fav_id);
        assert_eq!(state.items[3].id, reg_c);
        assert!(!state.items[3].is_favorite);
        assert!(state.selected_ids.is_empty());
    }

    #[test]
    fn range_select_replaces_the_entire_selection() {
        let (mut state, _dirty) = test_state();
        state.selected_ids = vec![1, 2, 3];
        // Ctrl/Cmd+A select-all hands the full visible id set to range_select,
        // which must replace (not merge) any prior selection wholesale.
        state.range_select(&[10, 20, 30, 40]);
        assert_eq!(state.selected_ids, vec![10, 20, 30, 40]);
    }

    #[test]
    fn favorites_beyond_scan_limit_are_not_found() {
        let (mut state, _dirty) = test_state();
        state.settings.search_favorites_first = true;
        // scan_limit = 200 × 8 = 1600 for the default result limit of 200;
        // the favorite below sits outside that window and must not appear.
        for i in 0..1601 {
            insert_item_at_age(&mut state, 2000 + i, false, "needle", 10 + i);
        }
        insert_item_at_age(&mut state, 3000, true, "needle fav-beyond", 5000);

        state.filters.set_keyword("needle");
        state.reload_items();

        assert_eq!(state.items.len(), 200);
        assert!(state.items.iter().all(|i| !i.is_favorite));
    }

    #[test]
    fn item_hotkey_update_is_local_metadata_only() {
        let (mut state, dirty) = test_state();
        dirty.store(false, Ordering::SeqCst);

        let mut item = make_item(1, ContentType::PlainText, true, "hello");
        item.created_at = chrono::Utc::now() - chrono::Duration::days(1);
        item.updated_at = item.created_at;
        let hash = item.content_hash;
        state.db.upsert(&item).unwrap();
        let db_item = state.db.get_by_hash(hash).unwrap().unwrap();
        let item_id = db_item.id;
        state.items.push(db_item.clone());

        let before_updated_at = db_item.updated_at;
        let format =
            serde_json::to_string(&crate::core::types::HotkeyPasteFormat::PlainText).unwrap();
        state.update_item_hotkey(item_id, "Ctrl+Alt+1", &format);

        let after_set = state.db.get_by_id(item_id).unwrap().unwrap();
        assert_eq!(after_set.updated_at, before_updated_at);
        assert_eq!(after_set.custom_hotkey, "Ctrl+Alt+1");
        assert_eq!(
            state.get_item_hotkey_format(item_id),
            crate::core::types::HotkeyPasteFormat::PlainText
        );
        assert!(!dirty.load(Ordering::SeqCst));

        state.clear_item_hotkey(item_id);
        let after_clear = state.db.get_by_id(item_id).unwrap().unwrap();
        assert_eq!(after_clear.updated_at, before_updated_at);
        assert!(after_clear.custom_hotkey.is_empty());
        assert!(!dirty.load(Ordering::SeqCst));
    }

    #[test]
    fn item_hotkey_format_accepts_legacy_variant_names() {
        let (mut state, _dirty) = test_state();
        let mut item = make_item(1, ContentType::PlainText, false, "hello");
        item.custom_hotkey_format = "Hex".to_string();
        state.items.push(item);

        assert_eq!(
            state.get_item_hotkey_format(1),
            crate::core::types::HotkeyPasteFormat::Hex
        );
    }

    #[test]
    fn latest_hotkey_slot_uses_unfiltered_latest_items() {
        let (mut state, _dirty) = test_state();
        let base = chrono::Utc::now() - chrono::Duration::hours(1);

        let mut older = make_item(1, ContentType::PlainText, false, "visible older");
        older.created_at = base;
        older.updated_at = base;
        let older_hash = older.content_hash;
        state.db.upsert(&older).unwrap();

        let mut newer = make_item(2, ContentType::PlainText, false, "hidden newer");
        newer.created_at = base + chrono::Duration::minutes(10);
        newer.updated_at = newer.created_at;
        let newer_hash = newer.content_hash;
        state.db.upsert(&newer).unwrap();

        let older_id = state.db.get_by_hash(older_hash).unwrap().unwrap().id;
        let newer_id = state.db.get_by_hash(newer_hash).unwrap().unwrap().id;

        state.filters.set_keyword("visible");
        state.reload_items();
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].id, older_id);

        assert_eq!(state.latest_hotkey_item_id(0), Some(newer_id));
        assert_eq!(state.latest_hotkey_item_id(1), Some(older_id));
    }

    // ── should_mark_sync_dirty ──────────────────────────────────────

    #[test]
    fn dirty_image_type_false_when_image_sync_disabled() {
        let (state, _dirty) = test_state();
        let item = make_item(1, ContentType::Image, false, "img.png");
        assert!(!state.should_mark_sync_dirty(&item));
    }

    #[test]
    fn dirty_image_type_true_when_image_sync_enabled() {
        let (mut state, _dirty) = test_state();
        state.settings.sync_include_images = true;
        state.settings.sync_favorites_only = false;
        let item = make_item(1, ContentType::Image, false, "img.png");
        assert!(state.should_mark_sync_dirty(&item));
    }

    #[test]
    fn dirty_file_type_always_false() {
        let (state, _dirty) = test_state();
        let item = make_item(1, ContentType::File, false, "doc.pdf");
        assert!(!state.should_mark_sync_dirty(&item));
    }

    #[test]
    fn transfer_backing_records_are_visible_only_in_transfer_view() {
        let (mut state, _dirty) = test_state();
        let ordinary = make_item(1, ContentType::File, false, "report.pdf");
        let mut backing = make_item(2, ContentType::File, false, "report.pdf");
        backing.meta_type = "transfer".into();
        backing.file_data = FileData {
            files: vec![crate::core::types::FileInfo {
                name: "report.pdf".into(),
                path: "C:\\cache\\report.pdf".into(),
                is_dir: false,
            }],
            transfer: true,
            remote_hash: "a".repeat(64),
        }
        .to_json();
        state.items = vec![ordinary.clone(), backing];

        let visible = state.visible_items();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, ordinary.id);

        state.transfer_filter_active = true;
        state.transfer_entries = vec![crate::core::transfer_types::ResolvedEntry {
            entry: crate::core::transfer_types::ManifestEntry {
                hash: "a".repeat(64),
                blob_id: String::new(),
                name: "report.pdf".into(),
                ext: "pdf".into(),
                size: 42,
                uploaded_at: chrono::Utc::now().to_rfc3339(),
                expires_at: String::new(),
                uploaded_by: "test".into(),
                pinned: false,
            },
            is_local: true,
            local_path: Some("C:\\cache\\report.pdf".into()),
        }];

        let visible = state.visible_items();
        assert_eq!(visible.len(), 1);
        assert!(visible[0].id < 0);
        assert!(FileData::from_json(&visible[0].file_data).is_transfer());

        state.toggle_type_filter("file");
        let visible_after_db_filter = state.visible_items();
        assert_eq!(visible_after_db_filter.len(), 1);
        assert!(visible_after_db_filter[0].id < 0);
    }

    #[test]
    fn transfer_download_is_tracked_per_entry_and_deduplicated() {
        let (mut state, _dirty) = test_state();
        let hash = "b".repeat(64);
        state.transfer_filter_active = true;
        state.transfer_entries = vec![crate::core::transfer_types::ResolvedEntry {
            entry: crate::core::transfer_types::ManifestEntry {
                hash: hash.clone(),
                blob_id: String::new(),
                name: "archive.zip".into(),
                ext: "zip".into(),
                size: 42,
                uploaded_at: chrono::Utc::now().to_rfc3339(),
                expires_at: String::new(),
                uploaded_by: "test".into(),
                pinned: false,
            },
            is_local: false,
            local_path: None,
        }];

        state.download_transfer_entry(&hash);
        state.download_transfer_entry(&hash);

        assert_eq!(state.pending_transfer_commands.len(), 1);
        assert!(state.pending_transfer_downloads.contains(&hash));
        let visible = state.visible_items();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].tags[0].name, I18nKey::TransferDownloading.text());
        assert_eq!(
            visible[0].tags[0].uid,
            crate::core::transfer_types::TRANSFER_STATUS_DOWNLOADING_UID
        );
    }

    #[test]
    fn transfer_upload_is_deduplicated_while_the_source_is_pending() {
        let (mut state, _dirty) = test_state();
        let directory = std::env::temp_dir().join(format!(
            "clippi-pending-upload-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("file.bin");
        std::fs::write(&path, b"test").unwrap();
        let mut item = make_item(0, ContentType::File, false, "file.bin");
        item.content_hash = 0xabcdef;
        item.file_data = FileData {
            files: vec![crate::core::types::FileInfo {
                name: "file.bin".into(),
                path: path.to_string_lossy().into_owned(),
                is_dir: false,
            }],
            ..Default::default()
        }
        .to_json();
        state.db.upsert(&item).unwrap();
        let item_id = state.db.get_by_hash(item.content_hash).unwrap().unwrap().id;
        state.settings.transfer_station_enabled = true;
        state
            .settings
            .sync_backends
            .push(crate::core::settings::BackendConfig {
                id: "local".into(),
                enabled: true,
                backend_type: "local_folder".into(),
                name: "local".into(),
                folder_path: directory.to_string_lossy().into_owned(),
                device_name: "test".into(),
                last_sync_at: String::new(),
                last_item_count: 0,
                last_tag_count: 0,
                sync_interval_secs: None,
                webdav_url: String::new(),
                webdav_root_url: String::new(),
                webdav_path: String::new(),
                webdav_username: String::new(),
                webdav_password: String::new(),
            });

        state.upload_to_transfer_station(item_id);
        state.upload_to_transfer_station(item_id);

        assert_eq!(state.pending_transfer_commands.len(), 1);
        assert!(state
            .pending_transfer_uploads
            .contains(path.to_string_lossy().as_ref()));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn deleting_local_db_file_changes_transfer_entry_to_cloud_only() {
        let (mut state, _dirty) = test_state();
        let directory = std::env::temp_dir().join(format!(
            "clippi-delete-local-transfer-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("download.bin");
        std::fs::write(&path, b"downloaded").unwrap();
        let path_text = path.to_string_lossy().into_owned();
        let remote_hash = "f".repeat(64);

        let mut local_item = make_item(0, ContentType::File, false, "download.bin");
        local_item.content_hash = 0x1010;
        local_item.file_data = FileData {
            files: vec![crate::core::types::FileInfo {
                name: "download.bin".into(),
                path: path_text.clone(),
                is_dir: false,
            }],
            transfer: false,
            remote_hash: remote_hash.clone(),
        }
        .to_json();
        state.db.upsert(&local_item).unwrap();
        let local_item = state.db.get_by_hash(0x1010).unwrap().unwrap();

        let mut backing_item = local_item.clone();
        backing_item.id = 0;
        backing_item.content_hash = 0x2020;
        backing_item.meta_type = "transfer".into();
        backing_item.file_data = FileData {
            files: vec![crate::core::types::FileInfo {
                name: "download.bin".into(),
                path: path_text.clone(),
                is_dir: false,
            }],
            transfer: true,
            remote_hash: remote_hash.clone(),
        }
        .to_json();
        state.db.upsert(&backing_item).unwrap();
        state.items.push(local_item.clone());
        state.transfer_entries = vec![crate::core::transfer_types::ResolvedEntry {
            entry: crate::core::transfer_types::ManifestEntry {
                hash: remote_hash,
                blob_id: String::new(),
                name: "download.bin".into(),
                ext: "bin".into(),
                size: 10,
                uploaded_at: chrono::Utc::now().to_rfc3339(),
                expires_at: String::new(),
                uploaded_by: "test".into(),
                pinned: false,
            },
            is_local: true,
            local_path: Some(path_text),
        }];

        state.delete_item(local_item.id);

        assert!(path.is_file());
        assert!(state.db.get_transfer_items().unwrap().is_empty());
        assert!(!state.transfer_entries[0].is_local);
        assert!(state.transfer_entries[0].local_path.is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn leaving_transfer_view_cancels_queued_automatic_reads_only() {
        let (mut state, _dirty) = test_state();
        state.transfer_filter_active = true;
        state
            .pending_transfer_commands
            .push_back(crate::services::transfer_station::TransferCommand::Refresh);
        state
            .pending_transfer_commands
            .push_back(crate::services::transfer_station::TransferCommand::Cleanup);
        state.pending_transfer_commands.push_back(
            crate::services::transfer_station::TransferCommand::Upload {
                source_path: "pending.bin".into(),
                file_name: "pending.bin".into(),
            },
        );

        state.toggle_transfer_filter();

        assert!(!state.transfer_filter_active);
        assert!(!state.transfer_refreshing);
        assert_eq!(state.pending_transfer_commands.len(), 2);
        assert!(state
            .pending_transfer_commands
            .iter()
            .any(|command| matches!(
                command,
                crate::services::transfer_station::TransferCommand::Cleanup
            )));
        assert!(matches!(
            state.pending_transfer_commands.back(),
            Some(crate::services::transfer_station::TransferCommand::Upload { .. })
        ));
    }

    #[test]
    fn toggle_transfer_refuses_without_enabled_backend() {
        // Enabled station but no sync backend at all: activation must fail,
        // show the toast, and leave the view state untouched so callers do
        // not switch pages or record a return view.
        let (mut state, _dirty) = test_state();
        state.settings.transfer_station_enabled = true;

        assert!(!state.toggle_transfer_filter());
        assert!(!state.transfer_filter_active);
        assert!(!state.transfer_refreshing);
        assert!(state.toast_message.is_some());
        assert!(state.pending_transfer_commands.is_empty());
    }

    #[test]
    fn toggle_transfer_refuses_when_selected_backend_disabled_or_deleted() {
        let (mut state, _dirty) = test_state();
        state.settings.transfer_station_enabled = true;
        state.settings.transfer_backend_id = "selected-id".to_string();
        state.settings.sync_backends = vec![backend_config("selected-id", true)];
        // The selected backend is disabled: no other backend may take over.
        state.settings.sync_backends[0].enabled = false;
        assert!(!state.toggle_transfer_filter());
        assert!(!state.transfer_filter_active);

        // The selected backend was deleted entirely.
        state.settings.sync_backends = vec![backend_config("other-id", true)];
        assert!(!state.toggle_transfer_filter());
        assert!(!state.transfer_filter_active);
    }

    #[test]
    fn toggle_transfer_activates_with_enabled_backend_and_reports_success() {
        let (mut state, _dirty) = test_state();
        state.settings.transfer_station_enabled = true;
        state.settings.sync_backends = vec![backend_config("backend-1", true)];

        assert!(state.toggle_transfer_filter());
        assert!(state.transfer_filter_active);
        assert!(state.transfer_refreshing);
        assert!(state.pending_transfer_commands.iter().any(|command| {
            matches!(
                command,
                crate::services::transfer_station::TransferCommand::Refresh
            )
        }));

        // Deactivating again always succeeds and reports success.
        assert!(state.toggle_transfer_filter());
        assert!(!state.transfer_filter_active);
    }

    #[test]
    fn toggle_transfer_keeps_station_off_when_station_disabled() {
        let (mut state, _dirty) = test_state();
        state.settings.transfer_station_enabled = false;
        state.settings.sync_backends = vec![backend_config("backend-1", true)];

        assert!(!state.toggle_transfer_filter());
        assert!(!state.transfer_filter_active);
        assert!(state.toast_message.is_some());
    }

    #[test]
    fn daily_transfer_cleanup_is_deduplicated() {
        let (mut state, _dirty) = test_state();

        state.queue_daily_transfer_cleanup();
        state.queue_daily_transfer_cleanup();

        assert_eq!(state.pending_transfer_commands.len(), 1);
        assert!(matches!(
            state.pending_transfer_commands.front(),
            Some(crate::services::transfer_station::TransferCommand::Cleanup)
        ));
    }

    #[test]
    fn transfer_batch_delete_queues_only_existing_entries() {
        let (mut state, _dirty) = test_state();
        let first_hash = "c".repeat(64);
        let second_hash = "d".repeat(64);
        state.transfer_entries = [first_hash.clone(), second_hash.clone()]
            .into_iter()
            .map(|hash| crate::core::transfer_types::ResolvedEntry {
                entry: crate::core::transfer_types::ManifestEntry {
                    hash,
                    blob_id: String::new(),
                    name: "file.bin".into(),
                    ext: "bin".into(),
                    size: 42,
                    uploaded_at: chrono::Utc::now().to_rfc3339(),
                    expires_at: String::new(),
                    uploaded_by: "test".into(),
                    pinned: false,
                },
                is_local: false,
                local_path: None,
            })
            .collect();

        state.delete_transfer_entries(&[first_hash, "e".repeat(64), second_hash]);

        assert_eq!(state.pending_transfer_commands.len(), 2);
        assert!(state
            .pending_transfer_commands
            .iter()
            .all(|command| matches!(
                command,
                crate::services::transfer_station::TransferCommand::Delete { .. }
            )));
    }

    #[test]
    fn transfer_search_filters_manifest_names_with_shared_rules() {
        let (mut state, _dirty) = test_state();
        state.transfer_filter_active = true;
        let mut entries = Vec::new();
        for (hash_byte, name) in [('a', "Railway-Order.pdf"), ('b', "工作计划.docx")] {
            entries.push(crate::core::transfer_types::ResolvedEntry {
                entry: crate::core::transfer_types::ManifestEntry {
                    hash: hash_byte.to_string().repeat(64),
                    blob_id: String::new(),
                    name: name.into(),
                    ext: "bin".into(),
                    size: 42,
                    uploaded_at: chrono::Utc::now().to_rfc3339(),
                    expires_at: String::new(),
                    uploaded_by: "DESKTOP-A".into(),
                    pinned: false,
                },
                is_local: false,
                local_path: None,
            });
        }
        state.transfer_entries = entries;

        state.set_keyword("rail order");
        let visible = state.visible_items();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].full_text, "Railway-Order.pdf");

        state.set_keyword("gzjh");
        let visible = state.visible_items();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].full_text, "工作计划.docx");

        // Only the file name is searched — upload device must not match.
        state.set_keyword("DESKTOP-A");
        assert!(state.visible_items().is_empty());

        state.set_keyword("");
        assert_eq!(state.visible_items().len(), 2);
    }

    #[test]
    fn transfer_keyword_input_does_not_reload_database_items() {
        let (mut state, _dirty) = test_state();
        // Seed the DB and populate self.items through a normal reload.
        let item = make_item(1, ContentType::PlainText, false, "数据库条目");
        state.db.upsert(&item).unwrap();
        state.reload_items();
        let ids_before: Vec<i64> = state.items.iter().map(|item| item.id).collect();
        assert!(!ids_before.is_empty());

        state.transfer_filter_active = true;
        state.transfer_entries = vec![crate::core::transfer_types::ResolvedEntry {
            entry: crate::core::transfer_types::ManifestEntry {
                hash: "a".repeat(64),
                blob_id: String::new(),
                name: "计划.txt".into(),
                ext: "txt".into(),
                size: 1,
                uploaded_at: chrono::Utc::now().to_rfc3339(),
                expires_at: String::new(),
                uploaded_by: "test".into(),
                pinned: false,
            },
            is_local: false,
            local_path: None,
        }];

        // Keystrokes in the transfer view must not touch the DB-backed list.
        state.set_keyword("计划");
        assert_eq!(
            state.items.iter().map(|item| item.id).collect::<Vec<_>>(),
            ids_before
        );
        let visible = state.visible_items();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].full_text, "计划.txt");

        state.set_keyword("");
        assert_eq!(
            state.items.iter().map(|item| item.id).collect::<Vec<_>>(),
            ids_before
        );
    }

    #[test]
    fn transfer_pin_commands_are_deduplicated_and_reject_missing_entries() {
        let (mut state, _dirty) = test_state();
        state.transfer_filter_active = true;
        let hash = "a".repeat(64);
        state.transfer_entries = vec![crate::core::transfer_types::ResolvedEntry {
            entry: crate::core::transfer_types::ManifestEntry {
                hash: hash.clone(),
                blob_id: String::new(),
                name: "pin.bin".into(),
                ext: "bin".into(),
                size: 1,
                uploaded_at: chrono::Utc::now().to_rfc3339(),
                expires_at: String::new(),
                uploaded_by: "test".into(),
                pinned: false,
            },
            is_local: false,
            local_path: None,
        }];

        state.set_transfer_entry_pinned(&hash, true);
        state.set_transfer_entry_pinned(&hash, false);
        assert_eq!(state.pending_transfer_commands.len(), 1);
        assert!(state.pending_transfer_pin_updates.contains(&hash));
        assert!(matches!(
            state.pending_transfer_commands.front(),
            Some(
                crate::services::transfer_station::TransferCommand::SetPinned { pinned: true, .. }
            )
        ));

        state.set_transfer_entry_pinned("e".repeat(64).as_str(), true);
        assert_eq!(state.pending_transfer_commands.len(), 1);
        assert_eq!(
            state.toast_message,
            Some(I18nKey::TransferEntryExpired.text().into())
        );
    }

    #[test]
    fn visible_items_expose_pinned_and_retention_metadata_tags() {
        let (mut state, _dirty) = test_state();
        state.transfer_filter_active = true;
        let expires_at = (chrono::Utc::now() + chrono::Duration::days(3)).to_rfc3339();
        state.transfer_entries = vec![crate::core::transfer_types::ResolvedEntry {
            entry: crate::core::transfer_types::ManifestEntry {
                hash: "a".repeat(64),
                blob_id: String::new(),
                name: "pin.bin".into(),
                ext: "bin".into(),
                size: 1,
                uploaded_at: chrono::Utc::now().to_rfc3339(),
                expires_at: expires_at.clone(),
                uploaded_by: "test".into(),
                pinned: true,
            },
            is_local: false,
            local_path: None,
        }];

        let visible = state.visible_items();
        assert_eq!(visible.len(), 1);
        let uids: Vec<&str> = visible[0].tags.iter().map(|tag| tag.uid.as_str()).collect();
        assert!(uids.contains(&crate::core::transfer_types::TRANSFER_STATUS_PINNED_UID));
        assert!(uids.contains(&crate::core::transfer_types::TRANSFER_STATUS_RETENTION_UID));
        let retention = visible[0]
            .tags
            .iter()
            .find(|tag| tag.uid == crate::core::transfer_types::TRANSFER_STATUS_RETENTION_UID)
            .unwrap();
        assert_eq!(retention.updated_at, expires_at);

        // Global retention off: stale explicit expirations are ignored too,
        // so the card can never show a countdown while cleanup is disabled.
        state.settings.transfer_retention_days = 0;
        let visible = state.visible_items();
        let retention = visible[0]
            .tags
            .iter()
            .find(|tag| tag.uid == crate::core::transfer_types::TRANSFER_STATUS_RETENTION_UID)
            .unwrap();
        assert!(retention.updated_at.is_empty());

        // ... and entries without any explicit expiry stay timeless as well.
        state.transfer_entries[0].entry.expires_at.clear();
        let visible = state.visible_items();
        let retention = visible[0]
            .tags
            .iter()
            .find(|tag| tag.uid == crate::core::transfer_types::TRANSFER_STATUS_RETENTION_UID)
            .unwrap();
        assert!(retention.updated_at.is_empty());
    }

    #[test]
    fn dirty_favorites_only_non_fav_false() {
        let (mut state, _dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, false, "hello");
        assert!(!state.should_mark_sync_dirty(&item));
    }

    #[test]
    fn dirty_favorites_only_fav_true() {
        let (mut state, _dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, true, "hello");
        assert!(state.should_mark_sync_dirty(&item));
    }

    #[test]
    fn dirty_normal_mode_non_fav_true() {
        let (mut state, _dirty) = test_state();
        state.settings.sync_favorites_only = false;
        let item = make_item(1, ContentType::PlainText, false, "hello");
        assert!(state.should_mark_sync_dirty(&item));
    }

    #[test]
    fn dirty_image_favorite_still_false_when_image_sync_disabled() {
        let (mut state, _dirty) = test_state();
        state.settings.sync_favorites_only = false;
        let item = make_item(1, ContentType::Image, true, "img.png");
        assert!(!state.should_mark_sync_dirty(&item));
    }

    // ── save_edited_item sync_dirty ────────────────────────────────

    #[test]
    fn save_edited_item_sets_dirty_in_normal_mode() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = false;
        // Insert item into DB and in-memory list
        let item = make_item(1, ContentType::PlainText, false, "original");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        let ok = state.save_edited_item(1, "edited", "plain");
        assert!(ok);
        assert!(
            dirty.load(Ordering::SeqCst),
            "normal mode: should set dirty"
        );
    }

    #[test]
    fn save_edited_item_sets_dirty_for_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, true, "original");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        let ok = state.save_edited_item(1, "edited", "plain");
        assert!(ok);
        assert!(
            dirty.load(Ordering::SeqCst),
            "favorite item in fav-only mode: should set dirty"
        );
    }

    #[test]
    fn save_edited_item_skips_dirty_for_non_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, false, "original");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        let ok = state.save_edited_item(1, "edited", "plain");
        assert!(ok);
        assert!(
            !dirty.load(Ordering::SeqCst),
            "non-favorite item in fav-only mode: should NOT set dirty"
        );
    }

    // ── toggle_item_tag sync_dirty ─────────────────────────────────

    fn setup_tag(state: &mut AppState) -> i64 {
        // Create a tag
        state.db.create_tag("test-tag", "#FF0000").unwrap()
    }

    // ── delete_tag ─────────────────────────────────

    #[test]
    fn delete_tag_removes_tag_and_associations_but_keeps_items() {
        let (mut state, _dirty) = test_state();
        let tag_id = setup_tag(&mut state);
        let item = make_item(1, ContentType::PlainText, false, "tagged");
        state.db.upsert(&item).unwrap();
        let item_id = state.db.get_by_hash(item.content_hash).unwrap().unwrap().id;
        state.db.add_item_tag(item_id, tag_id).unwrap();

        // Activate the tag filter and pin the tag in the sidebar.
        state.filters.tag_ids.push(tag_id);
        state.settings.pinned_tag_ids.push(tag_id);

        assert!(state.delete_tag(tag_id));

        // Tag is gone from the reloaded tag list.
        assert!(state.tags.iter().all(|t| t.id != tag_id));
        // Filter and pinned-sidebar entries were cleaned up.
        assert!(!state.filters.tag_ids.contains(&tag_id));
        assert!(!state.settings.pinned_tag_ids.contains(&tag_id));
        // The clipboard item survived; its tag association is gone.
        let item = state.db.get_by_id_with_tags(item_id).unwrap().unwrap();
        assert!(item.tags.is_empty());
    }

    #[test]
    fn delete_tag_writes_tombstone_and_marks_sync_dirty() {
        let (mut state, dirty) = test_state();
        let tag_id = state.db.create_tag("tomb-tag", "#00FF00").unwrap();
        let tag_uid = state.db.get_tag_by_id(tag_id).unwrap().unwrap().uid;
        assert!(!tag_uid.is_empty());

        assert!(!dirty.load(Ordering::SeqCst));
        assert!(state.delete_tag(tag_id));
        assert!(dirty.load(Ordering::SeqCst));
        // The tombstone is recorded under the tag's uid (local tags are not
        // uid-less).
        assert!(state.db.is_tag_tombstoned(&tag_uid, "tomb-tag").unwrap());
    }

    #[test]
    fn delete_tag_nonexistent_is_safe_and_keeps_other_tags() {
        let (mut state, _dirty) = test_state();
        let tag_id = setup_tag(&mut state);

        // Deleting a missing id is an idempotent no-op: it reports success,
        // leaves the other tag intact and writes no tombstone.
        assert!(state.delete_tag(9999));
        assert_eq!(state.tags.len(), 1);
        assert!(state.tags.iter().any(|t| t.id == tag_id));
        assert!(!state.db.is_tag_tombstoned("", "test-tag").unwrap());
    }

    #[test]
    fn keyword_search_scans_all_records_and_matches_rich_data_and_tags() {
        let (mut state, _dirty) = test_state();
        let now = chrono::Utc::now();

        for i in 0..205 {
            let mut item = make_item(i + 1, ContentType::PlainText, false, &format!("noise {i}"));
            item.created_at = now - chrono::Duration::seconds(i);
            item.updated_at = item.created_at;
            state.db.upsert(&item).unwrap();
        }

        let mut text_item = make_item(300, ContentType::PlainText, false, "buried plain match");
        text_item.created_at = now - chrono::Duration::days(10);
        text_item.updated_at = text_item.created_at;
        state.db.upsert(&text_item).unwrap();

        let mut ocr_item = make_item(301, ContentType::Image, false, "screenshot");
        ocr_item.rich_data = RichData {
            ocr_text: Some("buried ocr match".to_string()),
            ..Default::default()
        }
        .to_json();
        ocr_item.created_at = now - chrono::Duration::days(11);
        ocr_item.updated_at = ocr_item.created_at;
        state.db.upsert(&ocr_item).unwrap();

        let mut tagged_item = make_item(302, ContentType::PlainText, false, "tagged item");
        tagged_item.created_at = now - chrono::Duration::days(12);
        tagged_item.updated_at = tagged_item.created_at;
        state.db.upsert(&tagged_item).unwrap();
        let tagged_id = state
            .db
            .get_by_hash(tagged_item.content_hash)
            .unwrap()
            .unwrap()
            .id;
        let tag_id = state.db.create_tag("buried tag match", "#FF0000").unwrap();
        state.db.add_item_tag(tagged_id, tag_id).unwrap();

        state.filters.set_keyword("buried");
        state.reload_items();

        let texts: Vec<&str> = state
            .items
            .iter()
            .map(|item| item.full_text.as_str())
            .collect();
        assert!(texts.contains(&"buried plain match"));
        assert!(texts.contains(&"screenshot"));
        assert!(texts.contains(&"tagged item"));
    }

    #[test]
    fn keyword_search_requires_all_space_separated_terms() {
        let (mut state, _dirty) = test_state();
        state
            .db
            .upsert(&make_item(
                1,
                ContentType::PlainText,
                false,
                "railway ticket order",
            ))
            .unwrap();
        state
            .db
            .upsert(&make_item(
                2,
                ContentType::PlainText,
                false,
                "railway notice",
            ))
            .unwrap();

        state.filters.set_keyword("railway order");
        state.reload_items();

        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].full_text, "railway ticket order");
    }

    #[test]
    fn keyword_search_matches_cached_page_title() {
        let (mut state, _dirty) = test_state();
        let mut item = make_item(1, ContentType::PlainText, false, "https://example.com/a");
        item.meta_type = "link".to_string();
        item.rich_data = RichData {
            page_title: Some("Railway Order Details".to_string()),
            ..Default::default()
        }
        .to_json();
        state.db.upsert(&item).unwrap();

        state.filters.set_keyword("order details");
        state.reload_items();

        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].full_text, "https://example.com/a");
    }

    #[test]
    fn keyword_search_keeps_match_after_preview_truncation() {
        let (mut state, _dirty) = test_state();
        let content = format!("{} tailmatch", "x".repeat(LIST_FULL_TEXT_LIMIT + 512));
        let item = make_item(1, ContentType::PlainText, false, &content);
        state.db.upsert(&item).unwrap();

        state.filters.set_keyword("tailmatch");
        state.reload_items();

        assert_eq!(state.items.len(), 1);
        assert!(state.items[0].full_text.len() <= LIST_FULL_TEXT_LIMIT);
    }

    #[test]
    fn list_reload_keeps_preview_light_and_db_full_item_intact() {
        let (mut state, _dirty) = test_state();
        let full_text = "x".repeat(12_000);
        let html = format!("<p>{}</p>", "h".repeat(8_000));
        let mut item = make_item(1, ContentType::RichText, false, &full_text);
        item.meta_type = "html".to_string();
        item.rich_data = RichData {
            html: Some(html.clone()),
            page_title: Some("t".repeat(3_000)),
            ..Default::default()
        }
        .to_json();
        item.note = "n".repeat(3_000);
        item.source_app_name = "Example".to_string();
        item.source_app_icon = "a".repeat(10_000);
        let hash = item.content_hash;
        state.db.upsert(&item).unwrap();

        state.reload_items();

        assert_eq!(state.items.len(), 1);
        let preview = &state.items[0];
        assert!(preview.full_text.len() <= LIST_FULL_TEXT_LIMIT);
        assert!(preview.note.len() <= LIST_NOTE_LIMIT);
        assert_eq!(preview.source_app_icon.len(), 10_000);
        let rich_preview = RichData::from_json(&preview.rich_data);
        assert!(rich_preview.html.unwrap().len() <= LIST_RICH_HTML_LIMIT);
        assert!(rich_preview.page_title.unwrap().len() <= LIST_RICH_AUX_LIMIT);

        let full = state.db.get_by_hash(hash).unwrap().unwrap();
        assert_eq!(full.full_text.len(), full_text.len());
        assert_eq!(RichData::from_json(&full.rich_data).html.unwrap(), html);
        assert_eq!(full.source_app_icon.len(), 10_000);
    }

    #[test]
    fn keyword_search_html_uses_visible_text_not_markup_attributes() {
        let (mut state, _dirty) = test_state();
        let mut item = make_item(1, ContentType::RichText, false, "plain fallback");
        item.meta_type = "html".to_string();
        item.rich_data = RichData {
            html: Some(r#"<div data-key="hidden">plain visible text</div>"#.to_string()),
            ..Default::default()
        }
        .to_json();
        state.db.upsert(&item).unwrap();

        state.filters.set_keyword("key");
        state.reload_items();
        assert_eq!(state.items.len(), 0);

        state.filters.set_keyword("visible");
        state.reload_items();
        assert_eq!(state.items.len(), 1);
    }

    #[test]
    fn keyword_search_html_full_text_ignores_markup_attributes() {
        let (mut state, _dirty) = test_state();
        let mut item = make_item(
            1,
            ContentType::RichText,
            false,
            r#"<div data-key="api">plain visible text</div>"#,
        );
        item.meta_type = "html".to_string();
        state.db.upsert(&item).unwrap();

        state.filters.set_keyword("api");
        state.reload_items();

        assert_eq!(state.items.len(), 0);
    }

    #[test]
    fn keyword_search_html_full_text_matches_visible_substring() {
        let (mut state, _dirty) = test_state();
        let mut item = make_item(
            1,
            ContentType::RichText,
            false,
            r#"<div><span style="color:#bbbebf"> rapid=</span><span style="color:#569cd6">false</span></div>"#,
        );
        item.meta_type = "html".to_string();
        state.db.upsert(&item).unwrap();

        state.filters.set_keyword("api");
        state.reload_items();

        assert_eq!(state.items.len(), 1);
    }

    #[test]
    fn keyword_search_html_matches_visible_text_inside_rich_data() {
        let (mut state, _dirty) = test_state();
        let mut item = make_item(1, ContentType::RichText, false, "plain fallback");
        item.meta_type = "html".to_string();
        item.rich_data = RichData {
            html: Some(r#"<div><span style="color:#ff00aa">API Key</span></div>"#.to_string()),
            ..Default::default()
        }
        .to_json();
        state.db.upsert(&item).unwrap();

        state.filters.set_keyword("key");
        state.reload_items();

        assert_eq!(state.items.len(), 1);
    }

    #[test]
    fn keyword_search_matches_non_ascii_rich_text_outside_full_text() {
        let (mut state, _dirty) = test_state();
        let mut item = make_item(1, ContentType::RichText, false, "plain preview");
        item.rich_data = RichData {
            html: Some("<p>富文本命中</p>".to_string()),
            ..Default::default()
        }
        .to_json();
        state.db.upsert(&item).unwrap();

        state.filters.set_keyword("富文本");
        state.reload_items();

        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].full_text, "plain preview");
    }

    #[test]
    fn keyword_search_matches_note_with_pinyin_and_initials() {
        let (mut state, _dirty) = test_state();
        let item = make_item(1, ContentType::PlainText, false, "普通正文内容");
        state.db.upsert(&item).unwrap();
        let item_id = state.db.get_by_hash(item.content_hash).unwrap().unwrap().id;
        // 模拟真实流程：通过 update_note 写入备注
        state.db.update_note(item_id, "工作计划").unwrap();
        state.reload_items();

        // 直接文本匹配
        state.filters.set_keyword("工作");
        state.reload_items();
        assert_eq!(state.items.len(), 1, "直接文本匹配失败");

        // 全拼匹配: gongzuo
        state.filters.set_keyword("gongzuo");
        state.reload_items();
        assert_eq!(state.items.len(), 1, "全拼匹配失败");

        // 首字母匹配: gzjh
        state.filters.set_keyword("gzjh");
        state.reload_items();
        assert_eq!(state.items.len(), 1, "首字母全匹配失败");

        // 部分首字母匹配: gz
        state.filters.set_keyword("gz");
        state.reload_items();
        assert_eq!(state.items.len(), 1, "部分首字母匹配失败");

        // 确认备注字段中的中文匹配不依赖正文
        state.filters.set_keyword("计划");
        state.reload_items();
        assert_eq!(state.items.len(), 1, "备注中文匹配失败");

        // 确保未写入备注的不匹配
        state.filters.set_keyword("xyznotexist");
        state.reload_items();
        assert_eq!(state.items.len(), 0, "不应匹配的关键词却匹配了");
    }

    #[test]
    fn toggle_item_tag_sets_dirty_normal_mode() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = false;
        let tag_id = setup_tag(&mut state);
        let item = make_item(1, ContentType::PlainText, false, "hello");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.toggle_item_tag(1, tag_id);
        assert!(
            dirty.load(Ordering::SeqCst),
            "normal mode: should set dirty"
        );
    }

    #[test]
    fn toggle_item_tag_sets_dirty_for_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let tag_id = setup_tag(&mut state);
        let item = make_item(1, ContentType::PlainText, true, "hello");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.toggle_item_tag(1, tag_id);
        assert!(
            dirty.load(Ordering::SeqCst),
            "favorite item in fav-only mode: should set dirty"
        );
    }

    #[test]
    fn toggle_item_tag_skips_dirty_for_non_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let tag_id = setup_tag(&mut state);
        let item = make_item(1, ContentType::PlainText, false, "hello");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.toggle_item_tag(1, tag_id);
        assert!(
            !dirty.load(Ordering::SeqCst),
            "non-favorite item in fav-only mode: should NOT set dirty"
        );
    }

    // ── clear_item_tags sync_dirty ─────────────────────────────────

    #[test]
    fn clear_item_tags_skips_dirty_for_non_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let tag_id = setup_tag(&mut state);
        let item = make_item(1, ContentType::PlainText, false, "hello");
        state.db.upsert(&item).unwrap();
        state.db.add_item_tag(1, tag_id).unwrap();
        state.items.push(item);

        state.clear_item_tags(1);
        assert!(
            !dirty.load(Ordering::SeqCst),
            "non-favorite item in fav-only mode: should NOT set dirty"
        );
    }

    #[test]
    fn clear_item_tags_sets_dirty_for_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let tag_id = setup_tag(&mut state);
        let item = make_item(1, ContentType::PlainText, true, "hello");
        state.db.upsert(&item).unwrap();
        state.db.add_item_tag(1, tag_id).unwrap();
        state.items.push(item);

        state.clear_item_tags(1);
        assert!(
            dirty.load(Ordering::SeqCst),
            "favorite item in fav-only mode: should set dirty"
        );
    }

    // ── toggle_favorite ALWAYS sets dirty ──────────────────────────

    #[test]
    fn toggle_favorite_always_sets_dirty_even_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, false, "hello");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.toggle_favorite(1);
        assert!(
            dirty.load(Ordering::SeqCst),
            "toggle_favorite should always set dirty (tombstones)"
        );
    }

    #[test]
    fn delete_item_skips_tombstone_outside_sync_scope() {
        // favorites_only=true, item not favorited → outside sync scope
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, false, "hello");
        let hash = item.content_hash;
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.delete_item(1);
        assert!(
            !state.db.is_item_tombstoned(hash).unwrap(),
            "delete outside sync scope should NOT record tombstone"
        );
        assert!(
            !dirty.load(Ordering::SeqCst),
            "delete outside sync scope should NOT set dirty (no tombstone needed)"
        );
    }

    #[test]
    fn delete_item_records_tombstone_in_sync_scope() {
        // Normal mode: PlainText item → in sync scope
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = false;
        let item = make_item(1, ContentType::PlainText, false, "hello");
        let hash = item.content_hash;
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.delete_item(1);
        assert!(
            state.db.is_item_tombstoned(hash).unwrap(),
            "delete in sync scope should record tombstone"
        );
        assert!(
            dirty.load(Ordering::SeqCst),
            "delete in sync scope should set dirty (tombstone)"
        );
    }

    #[test]
    fn delete_item_skips_tombstone_for_image_when_image_sync_disabled() {
        let (mut state, dirty) = test_state();
        let item = make_item(1, ContentType::Image, false, "image_data");
        let hash = item.content_hash;
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.delete_item(1);
        assert!(
            !state.db.is_item_tombstoned(hash).unwrap(),
            "delete image should NOT record tombstone"
        );
        assert!(
            !dirty.load(Ordering::SeqCst),
            "delete image should NOT set dirty"
        );
    }

    #[test]
    fn delete_item_records_tombstone_for_image_when_image_sync_enabled() {
        let (mut state, dirty) = test_state();
        state.settings.sync_include_images = true;
        state.settings.sync_favorites_only = false;
        let item = make_item(1, ContentType::Image, false, "image_data");
        let hash = item.content_hash;
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.delete_item(1);
        assert!(
            state.db.is_item_tombstoned(hash).unwrap(),
            "delete synced image should record tombstone"
        );
        assert!(
            dirty.load(Ordering::SeqCst),
            "delete synced image should set dirty"
        );
    }

    #[test]
    fn batch_delete_records_tombstones_only_for_sync_scope() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;

        let favorite = make_item(1, ContentType::PlainText, true, "favorite");
        let non_favorite = make_item(2, ContentType::PlainText, false, "non-favorite");
        let image = make_item(3, ContentType::Image, true, "image_data");
        let favorite_hash = favorite.content_hash;
        let non_favorite_hash = non_favorite.content_hash;
        let image_hash = image.content_hash;

        state.db.upsert(&favorite).unwrap();
        state.db.upsert(&non_favorite).unwrap();
        state.db.upsert(&image).unwrap();
        let favorite_id = state.db.get_by_hash(favorite_hash).unwrap().unwrap().id;
        let non_favorite_id = state.db.get_by_hash(non_favorite_hash).unwrap().unwrap().id;
        let image_id = state.db.get_by_hash(image_hash).unwrap().unwrap().id;
        state.db.set_favorite(favorite_id, true).unwrap();
        state.db.set_favorite(image_id, true).unwrap();
        state.items.extend([favorite, non_favorite, image]);
        state.selected_ids = vec![favorite_id, non_favorite_id, image_id];

        state.batch_delete();

        assert!(state.db.is_item_tombstoned(favorite_hash).unwrap());
        assert!(!state.db.is_item_tombstoned(non_favorite_hash).unwrap());
        assert!(!state.db.is_item_tombstoned(image_hash).unwrap());
        assert!(
            dirty.load(Ordering::SeqCst),
            "batch delete should set dirty when at least one selected item is in sync scope"
        );
    }

    #[test]
    fn batch_delete_skips_dirty_and_tombstones_when_all_items_outside_sync_scope() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;

        let non_favorite = make_item(1, ContentType::PlainText, false, "non-favorite");
        let image = make_item(2, ContentType::Image, true, "image_data");
        let non_favorite_hash = non_favorite.content_hash;
        let image_hash = image.content_hash;

        state.db.upsert(&non_favorite).unwrap();
        state.db.upsert(&image).unwrap();
        let non_favorite_id = state.db.get_by_hash(non_favorite_hash).unwrap().unwrap().id;
        let image_id = state.db.get_by_hash(image_hash).unwrap().unwrap().id;
        state.db.set_favorite(image_id, true).unwrap();
        state.items.extend([non_favorite, image]);
        state.selected_ids = vec![non_favorite_id, image_id];

        state.batch_delete();

        assert!(!state.db.is_item_tombstoned(non_favorite_hash).unwrap());
        assert!(!state.db.is_item_tombstoned(image_hash).unwrap());
        assert!(
            !dirty.load(Ordering::SeqCst),
            "batch delete outside sync scope should NOT set dirty"
        );
    }

    #[test]
    fn batch_delete_queues_custom_hotkeys_for_unregister() {
        let (mut state, _dirty) = test_state();

        let mut item = make_item(1, ContentType::PlainText, false, "hotkey item");
        item.custom_hotkey = "Ctrl+Alt+1".to_string();
        let hash = item.content_hash;
        state.db.upsert(&item).unwrap();
        let item_id = state.db.get_by_hash(hash).unwrap().unwrap().id;
        state.db.set_item_hotkey(item_id, "Ctrl+Alt+1", "").unwrap();
        state
            .items
            .push(state.db.get_by_id(item_id).unwrap().unwrap());
        state.selected_ids = vec![item_id];

        state.batch_delete();

        assert_eq!(state.pending_hotkey_unregister, vec![item_id]);
    }

    #[test]
    fn merge_selected_items_rejects_non_text_items() {
        let (mut state, _dirty) = test_state();
        let text = make_item(1, ContentType::PlainText, false, "text");
        let image = make_item(2, ContentType::Image, false, "image_data");
        let text_hash = text.content_hash;
        let image_hash = image.content_hash;

        state.db.upsert(&text).unwrap();
        state.db.upsert(&image).unwrap();
        let text_id = state.db.get_by_hash(text_hash).unwrap().unwrap().id;
        let image_id = state.db.get_by_hash(image_hash).unwrap().unwrap().id;
        state.reload_items();
        state.selected_ids = vec![text_id, image_id];

        assert!(!state.can_merge_selected_items());
        assert!(state.merge_selected_items().is_none());
        state.reload_items();
        assert_eq!(state.items.len(), 2);
    }

    #[test]
    fn merge_selected_items_creates_new_entry_even_when_text_already_exists() {
        let (mut state, _dirty) = test_state();
        let existing = make_item(1, ContentType::PlainText, false, "ab");
        let first = make_item(2, ContentType::PlainText, false, "a");
        let second = make_item(3, ContentType::PlainText, false, "b");
        let existing_hash = existing.content_hash;
        let first_hash = first.content_hash;
        let second_hash = second.content_hash;

        state.db.upsert(&existing).unwrap();
        state.db.upsert(&first).unwrap();
        state.db.upsert(&second).unwrap();
        let existing_id = state.db.get_by_hash(existing_hash).unwrap().unwrap().id;
        let first_id = state.db.get_by_hash(first_hash).unwrap().unwrap().id;
        let second_id = state.db.get_by_hash(second_hash).unwrap().unwrap().id;
        state.reload_items();
        state.selected_ids = vec![first_id, second_id];

        assert!(state.can_merge_selected_items());
        let merged_id = state.merge_selected_items().unwrap();

        assert_ne!(merged_id, existing_id);
        let merged = state.db.get_by_id(merged_id).unwrap().unwrap();
        assert_eq!(merged.full_text, "ab");
        assert_eq!(
            state
                .items
                .iter()
                .filter(|item| item.full_text == "ab")
                .count(),
            2
        );
    }

    // ── update_note sync_dirty ─────────────────────────────────────

    #[test]
    fn update_note_sets_dirty_in_normal_mode() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = false;
        let item = make_item(1, ContentType::PlainText, true, "hello");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.update_note(1, "new note");
        assert!(
            dirty.load(Ordering::SeqCst),
            "normal mode: should set dirty"
        );
    }

    #[test]
    fn update_note_skips_dirty_for_non_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, false, "hello");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.update_note(1, "new note");
        assert!(
            !dirty.load(Ordering::SeqCst),
            "non-favorite in fav-only: should NOT set dirty"
        );
    }

    #[test]
    fn update_note_sets_dirty_for_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, true, "hello");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.update_note(1, "new note");
        assert!(
            dirty.load(Ordering::SeqCst),
            "favorite in fav-only: should set dirty"
        );
    }

    // ── P0: usage-path performance baseline ────────────────────────
    // Machine-timed, ignored by default (run with --ignored --nocapture).
    // Measures single/batch usage-time updates against a 10,000-item
    // history. The P1 signature change of touch_item_usage adapts the call
    // sites below; scenarios and statistics stay unchanged.

    fn baseline_median_us(samples: &[u128]) -> u128 {
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    }

    /// Warm up (2 runs), then measure `runs` iterations; returns (mean, median) µs.
    fn baseline_scenario<F: FnMut()>(mut op: F, runs: usize) -> (u128, u128) {
        for _ in 0..2 {
            op();
        }
        let mut samples = Vec::with_capacity(runs);
        for _ in 0..runs {
            let start = std::time::Instant::now();
            op();
            samples.push(start.elapsed().as_micros());
        }
        let mean = samples.iter().sum::<u128>() / samples.len() as u128;
        (mean, baseline_median_us(&samples))
    }

    #[test]
    #[ignore = "machine-timed usage-path baseline; run with --ignored --nocapture"]
    fn usage_performance_baseline() {
        let (mut state, _dirty) = test_state();
        let mut ids = Vec::with_capacity(10_000);
        for i in 0..10_000i64 {
            ids.push(insert_item_at_age(
                &mut state,
                1000 + i,
                false,
                &format!("usage item {i}"),
                10_000 - i,
            ));
        }
        state.reload_items();
        assert_eq!(state.items.len(), 200, "default list window is 200");

        // Reload-detection signal: delete the oldest item (outside the list
        // window) without reloading. A full reload during a usage update
        // refreshes clearable_history_count from 10000 to 9999.
        state.db.delete_item(ids[0]).unwrap();
        let stale_count = state.clearable_history_count;
        assert_eq!(stale_count, 10_000);

        let newest_id = state.items[0].id;
        let newest_item = state.db.get_by_id(newest_id).unwrap().unwrap();
        let runs = 10;

        // Default updated_at order: single touch.
        let (mean, median) = baseline_scenario(|| state.touch_item_usage(&newest_item), runs);
        let reload_detected = state.clearable_history_count != stale_count;
        println!(
            "single touch (updated_at order): mean {mean} µs, median {median} µs, full reload detected: {reload_detected}"
        );

        // created_at order: positions must not change.
        state.settings.sort_by_created = true;
        state.reload_items();
        let before: Vec<i64> = state.items.iter().map(|it| it.id).collect();
        let (mean, median) = baseline_scenario(|| state.touch_item_usage(&newest_item), runs);
        let after: Vec<i64> = state.items.iter().map(|it| it.id).collect();
        println!(
            "single touch (created_at order): mean {mean} µs, median {median} µs, order unchanged: {}",
            before == after
        );

        // Favorites-first keyword search: touch inside the favorite group.
        state.settings.sort_by_created = false;
        state.settings.search_favorites_first = true;
        state.filters.set_keyword("usage item 9");
        state.reload_items();
        let fav_id = state.items[0].id;
        state.db.toggle_favorite(fav_id).unwrap();
        state.reload_items();
        assert_eq!(state.items[0].id, fav_id, "favorite leads the group");
        let fav_item = state.db.get_by_id(fav_id).unwrap().unwrap();
        let (mean, median) = baseline_scenario(|| state.touch_item_usage(&fav_item), runs);
        println!("single touch (favorites-first search): mean {mean} µs, median {median} µs");

        // Batch usage updates (the shared batch method used by batch_paste).
        let batch_20: Vec<ClipboardItem> = state
            .items
            .iter()
            .take(20)
            .filter_map(|it| state.db.get_by_id(it.id).ok().flatten())
            .collect();
        let batch_100: Vec<ClipboardItem> = state
            .items
            .iter()
            .take(100)
            .filter_map(|it| state.db.get_by_id(it.id).ok().flatten())
            .collect();
        let (mean, median) = baseline_scenario(|| state.touch_items_usage(&batch_20), runs);
        println!("batch usage update (20 items): mean {mean} µs, median {median} µs");
        let (mean, median) = baseline_scenario(|| state.touch_items_usage(&batch_100), runs);
        println!("batch usage update (100 items): mean {mean} µs, median {median} µs");
    }

    // ── Usage-path incremental updates (P1/P2) ─────────────────────

    #[test]
    fn usage_touch_moves_item_to_front_in_updated_at_order() {
        let (mut state, _dirty) = test_state();
        let id_old = insert_item_at_age(&mut state, 1, false, "oldest", 100);
        insert_item_at_age(&mut state, 2, false, "middle", 50);
        insert_item_at_age(&mut state, 3, false, "newest", 10);
        state.reload_items();
        assert_eq!(item_texts(&state), vec!["newest", "middle", "oldest"]);

        let item = state.db.get_by_id(id_old).unwrap().unwrap();
        state.touch_item_usage(&item);

        assert_eq!(item_texts(&state), vec!["oldest", "newest", "middle"]);
        assert_eq!(
            state.items[0].updated_at,
            state.db.get_by_id(id_old).unwrap().unwrap().updated_at
        );
        // The touched id is recorded for the list view to consume.
        assert_eq!(state.last_usage_touched_ids, vec![id_old]);
        assert_eq!(state.take_usage_sync_request(), (vec![id_old], false));
        assert!(state.last_usage_touched_ids.is_empty());
    }

    #[test]
    fn usage_sync_request_accumulates_until_consumed() {
        let (mut state, _dirty) = test_state();
        let id_a = insert_item_at_age(&mut state, 1, false, "a", 20);
        let id_b = insert_item_at_age(&mut state, 2, false, "b", 10);
        state.reload_items();

        let item_a = state.db.get_by_id(id_a).unwrap().unwrap();
        state.touch_item_usage(&item_a);
        let item_b = state.db.get_by_id(id_b).unwrap().unwrap();
        state.touch_item_usage(&item_b);
        // Touching the same item again must not duplicate the pending id.
        state.touch_item_usage(&item_a);

        assert_eq!(state.take_usage_sync_request(), (vec![id_a, id_b], false));
        assert_eq!(state.take_usage_sync_request(), (Vec::new(), false));
    }

    #[test]
    fn full_usage_sync_request_supersedes_pending_ids() {
        let (mut state, _dirty) = test_state();
        state.last_usage_touched_ids = vec![1, 2];
        state.usage_sync_requires_full_reload = true;

        assert_eq!(state.take_usage_sync_request(), (vec![1, 2], true));
        assert_eq!(state.take_usage_sync_request(), (Vec::new(), false));

        state.last_usage_touched_ids = vec![3];
        state.usage_sync_requires_full_reload = true;
        state.clear_usage_sync_request();
        assert_eq!(state.take_usage_sync_request(), (Vec::new(), false));
    }

    #[test]
    fn usage_touch_keeps_order_in_created_at_sort() {
        let (mut state, _dirty) = test_state();
        let id = insert_item_at_age(&mut state, 1, false, "a", 100);
        insert_item_at_age(&mut state, 2, false, "b", 50);
        insert_item_at_age(&mut state, 3, false, "c", 10);
        state.settings.sort_by_created = true;
        state.reload_items();
        let before: Vec<i64> = state.items.iter().map(|it| it.id).collect();

        let item = state.db.get_by_id(id).unwrap().unwrap();
        state.touch_item_usage(&item);

        let after: Vec<i64> = state.items.iter().map(|it| it.id).collect();
        assert_eq!(before, after);
        let touched = state.items.iter().find(|it| it.id == id).unwrap();
        assert!(touched.updated_at > touched.created_at);
    }

    #[test]
    fn usage_touch_reorders_only_inside_favorites_first_groups() {
        let (mut state, _dirty) = test_state();
        state.settings.search_favorites_first = true;
        let fav_old = insert_item_at_age(&mut state, 1, true, "k fav old", 100);
        insert_item_at_age(&mut state, 2, false, "k reg new", 10);
        insert_item_at_age(&mut state, 3, true, "k fav new", 50);
        insert_item_at_age(&mut state, 4, false, "k reg old", 200);
        state.filters.set_keyword("k");
        state.reload_items();
        assert_eq!(
            item_texts(&state),
            vec!["k fav new", "k fav old", "k reg new", "k reg old"]
        );

        let item = state.db.get_by_id(fav_old).unwrap().unwrap();
        state.touch_item_usage(&item);

        // The favorite moved inside its group; the regular group is untouched.
        assert_eq!(
            item_texts(&state),
            vec!["k fav old", "k fav new", "k reg new", "k reg old"]
        );
    }

    #[test]
    fn usage_touch_never_inserts_items_outside_current_result() {
        let (mut state, _dirty) = test_state();
        insert_item_at_age(&mut state, 1, false, "k visible one", 100);
        insert_item_at_age(&mut state, 2, false, "k visible two", 50);
        let hidden = insert_item_at_age(&mut state, 99, false, "hidden ghost", 10);
        state.filters.set_keyword("k");
        state.reload_items();
        let before: Vec<i64> = state.items.iter().map(|it| it.id).collect();
        assert!(!before.contains(&hidden));

        let item = state.db.get_by_id(hidden).unwrap().unwrap();
        state.touch_item_usage(&item);

        let after: Vec<i64> = state.items.iter().map(|it| it.id).collect();
        assert_eq!(before, after);
        // The database side is updated even though the list result excludes it.
        let db_updated = state.db.get_by_id(hidden).unwrap().unwrap().updated_at;
        let now = chrono::Utc::now();
        assert!((now - db_updated).num_seconds().abs() < 5);
        assert_eq!(state.last_usage_touched_ids, vec![hidden]);
    }

    #[test]
    fn batch_usage_touch_dedupes_ids_and_reorders_once() {
        let (mut state, _dirty) = test_state();
        let id_a = insert_item_at_age(&mut state, 1, false, "a", 100);
        let id_b = insert_item_at_age(&mut state, 2, false, "b", 50);
        let id_c = insert_item_at_age(&mut state, 3, false, "c", 10);
        state.reload_items();
        assert_eq!(item_texts(&state), vec!["c", "b", "a"]);

        // Duplicate ids in the batch must not duplicate in-memory entries.
        let items: Vec<ClipboardItem> = [id_a, id_b, id_c, id_a]
            .iter()
            .filter_map(|&id| state.db.get_by_id(id).ok().flatten())
            .collect();
        state.touch_items_usage(&items);

        assert_eq!(state.items.len(), 3);
        assert_eq!(item_texts(&state), vec!["c", "b", "a"]); // stable: all share the new timestamp
        let times: std::collections::HashSet<chrono::DateTime<chrono::Utc>> =
            state.items.iter().map(|it| it.updated_at).collect();
        assert_eq!(times.len(), 1);
        assert_eq!(state.last_usage_touched_ids, vec![id_a, id_b, id_c]);
    }

    #[test]
    fn usage_touch_marks_sync_dirty_by_sync_scope() {
        // Plain text (sync not favorites-only): in sync scope → dirty.
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = false;
        let id = insert_item_at_age(&mut state, 1, false, "text", 10);
        state.reload_items();
        let item = state.db.get_by_id(id).unwrap().unwrap();
        state.touch_item_usage(&item);
        assert!(dirty.load(Ordering::SeqCst));

        // File items are never synced.
        let (mut state2, dirty2) = test_state();
        let file = make_item(2, ContentType::File, false, "file");
        state2.db.upsert(&file).unwrap();
        let file_id = state2
            .db
            .get_by_hash(file.content_hash)
            .unwrap()
            .unwrap()
            .id;
        state2.reload_items();
        let file_item = state2.db.get_by_id(file_id).unwrap().unwrap();
        state2.touch_item_usage(&file_item);
        assert!(!dirty2.load(Ordering::SeqCst));

        // Favorites-only sync: non-favorite stays clean, favorite turns dirty.
        let (mut state3, dirty3) = test_state();
        state3.settings.sync_favorites_only = true;
        let plain = insert_item_at_age(&mut state3, 4, false, "plain", 20);
        state3.reload_items();
        let plain_item = state3.db.get_by_id(plain).unwrap().unwrap();
        state3.touch_item_usage(&plain_item);
        assert!(!dirty3.load(Ordering::SeqCst));
        let fav = insert_item_at_age(&mut state3, 3, true, "fav", 10);
        state3.reload_items();
        let fav_item = state3.db.get_by_id(fav).unwrap().unwrap();
        state3.touch_item_usage(&fav_item);
        assert!(dirty3.load(Ordering::SeqCst));
    }

    #[test]
    fn usage_touch_failure_keeps_memory_unchanged() {
        // DB-level failure atomicity is covered in db.rs; here we verify the
        // in-memory half stays untouched when the database rejects the write.
        let (mut state, dirty) = test_state();
        let id = insert_item_at_age(&mut state, 1, false, "text", 10);
        state.reload_items();
        let before: Vec<i64> = state.items.iter().map(|it| it.id).collect();
        let before_updated = state.items[0].updated_at;

        // Reject the next UPDATE on clipboard_items via a trigger.
        state.db.reject_updated_at_updates_for_test();
        let item = state.db.get_by_id(id).unwrap().unwrap();
        state.touch_item_usage(&item);

        let after: Vec<i64> = state.items.iter().map(|it| it.id).collect();
        assert_eq!(before, after);
        assert_eq!(state.items[0].updated_at, before_updated);
        assert!(!dirty.load(Ordering::SeqCst));
        assert!(state.last_usage_touched_ids.is_empty());
    }
}
