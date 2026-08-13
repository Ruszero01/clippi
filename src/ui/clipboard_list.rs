//! --- Clipboard list — variable-height virtual scrolling list. ---
//!
//! --- Uses `gpui_component::v_virtual_list` to efficiently render thousands ---
//! --- of clipboard items with dynamic card heights (68— 28px). ---

use std::{collections::HashSet, rc::Rc};

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::InputState;
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use gpui_component::v_virtual_list;
use gpui_component::VirtualListScrollHandle;

use crate::core::i18n_keys::I18nKey;
use crate::core::types::{ClipboardItem, ContentType, FileData, HotkeyPasteFormat};
use crate::state::app::AppState;

use super::clipboard_card::{estimate_card_height, ClipboardCard};
use super::search_box::SearchBox;
use super::tag_picker::TagState;
use super::theme::ClippiTheme;

const CLIPBOARD_ROW_VERTICAL_SPACE: f32 = 12.0;
const CLIPBOARD_BOTTOM_SCROLL_INSET: f32 = 36.0;
const CLIPBOARD_SCROLLBAR_WIDTH: f32 = 16.0;
const TAG_PICKER_WIDTH: f32 = 304.0;
const TAG_PICKER_CARD_ANCHOR_Y: f32 = 10.0;
const TAG_PICKER_FALLBACK_X: f32 = 400.0;
const TAG_PICKER_FALLBACK_Y: f32 = 80.0;

fn additive_selection_modifier(modifiers: Modifiers) -> bool {
    modifiers.control || modifiers.secondary()
}

fn primary_modifier_pressed(modifiers: Modifiers) -> bool {
    modifiers.secondary()
}

/// Stable pairwise reorder for usage updates, mirroring the AppState usage
/// reorder rule: created_at ordering keeps positions; favorites-first keyword
/// searches reorder only inside each group. Items move together with their
/// cached heights so index-based card sizes stay correct.
fn reorder_usage_pairs(
    pairs: &mut [(ClipboardItem, gpui::Size<gpui::Pixels>)],
    sort_by_created: bool,
    favorites_first: bool,
) {
    if sort_by_created || pairs.is_empty() {
        return;
    }
    if favorites_first {
        let split = pairs
            .iter()
            .position(|(item, _)| !item.is_favorite)
            .unwrap_or(pairs.len());
        pairs[..split].sort_by_key(|(item, _)| std::cmp::Reverse(item.updated_at));
        pairs[split..].sort_by_key(|(item, _)| std::cmp::Reverse(item.updated_at));
    } else {
        pairs.sort_by_key(|(item, _)| std::cmp::Reverse(item.updated_at));
    }
}

fn can_incrementally_sync_usage(
    local_items: &[ClipboardItem],
    item_sizes_len: usize,
    app_items: &[ClipboardItem],
    touched: &[i64],
    transfer_filter_active: bool,
    force_full_reload: bool,
) -> bool {
    if transfer_filter_active
        || force_full_reload
        || touched.is_empty()
        || item_sizes_len != local_items.len()
    {
        return false;
    }

    let local_ids: HashSet<i64> = local_items.iter().map(|item| item.id).collect();
    let app_visible_ids: HashSet<i64> = app_items
        .iter()
        .filter(|item| {
            item.content_type != ContentType::File
                || !FileData::from_json(&item.file_data).is_transfer()
        })
        .map(|item| item.id)
        .collect();

    local_ids.len() == local_items.len()
        && app_visible_ids.len() == local_items.len()
        && local_ids == app_visible_ids
        && touched.iter().all(|id| local_ids.contains(id))
}

fn item_id_at(items: &[ClipboardItem], index: Option<usize>) -> Option<i64> {
    index.and_then(|index| items.get(index)).map(|item| item.id)
}

fn item_index(items: &[ClipboardItem], id: Option<i64>) -> Option<usize> {
    id.and_then(|id| items.iter().position(|item| item.id == id))
}

#[cfg(target_os = "macos")]
fn macos_control_modifier_pressed() -> bool {
    objc2_app_kit::NSEvent::modifierFlags_class()
        .contains(objc2_app_kit::NSEventModifierFlags::Control)
}

/// Types of confirmation dialogs that can be shown.
/// [FUTURE] Add variants here for other confirmation scenarios
/// (e.g. RemoveBlacklist { app_name: String } for hotkey settings).
#[derive(Clone)]
pub(crate) enum ConfirmDialogState {
    Single { id: i64 },
    Batch { count: usize },
    Transfer { hash: String },
    TransferBatch { hashes: Vec<String> },
}

pub enum ClipboardListEvent {
    OpenEdit(i64),
    RequestHide,
}

/// Modifier kind for a modified double-click paste candidate.
enum ModifiedPasteModifier {
    Bitmap,
    PlainText,
}

/// One-shot candidate for modified double-click paste, established when a
/// first modified click leaves the selection set unchanged.
struct ModifiedDoubleClickCandidate {
    item_id: i64,
    modifier: ModifiedPasteModifier,
}

/// Normalized modifier classification for modified double-click gestures.
///
/// Only a bare primary modifier (Ctrl on Windows/Linux, Cmd on macOS) or a
/// bare Shift forms a paste kind. Any extra modifier (Alt, Fn, Win/Super, …)
/// or combination (Ctrl+Shift, …) yields `None`, so those gestures can never
/// establish a candidate or paste on the second click. Shared by candidate
/// formation and double-click matching so both sides stay consistent.
fn modified_double_click_kind(modifiers: Modifiers) -> Option<ModifiedPasteModifier> {
    if modifiers.number_of_modifiers() != 1 {
        return None;
    }
    if primary_modifier_pressed(modifiers) {
        Some(ModifiedPasteModifier::Bitmap)
    } else if modifiers.shift {
        Some(ModifiedPasteModifier::PlainText)
    } else {
        None
    }
}

impl EventEmitter<ClipboardListEvent> for ClipboardListView {}

/// The clipboard list view entity.
pub struct ClipboardListView {
    items: Vec<ClipboardItem>,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    card_height_mode: String,
    scroll_handle: VirtualListScrollHandle,
    focus_handle: FocusHandle,
    selected_ids: Vec<i64>,
    selected_index: Option<usize>,
    anchor_index: Option<usize>,
    state: Entity<AppState>,
    /// Reference to the search box for programmatic focus (Ctrl+F).
    /// Set after construction to resolve circular dependency.
    pub(crate) search_bar: Option<Entity<SearchBox>>,
    // --- Hover tracking ---
    hovered_index: Option<usize>,
    last_hover_pos: Option<Point<Pixels>>,
    // --- Context menu state ---
    context_menu_visible: bool,
    context_menu_x: f32,
    context_menu_y: f32,
    context_menu_item: Option<ClipboardItem>,
    context_menu_is_batch: bool,
    tag_picker_visible: bool,
    tag_picker_x: f32,
    tag_picker_y: f32,
    tag_picker_item_id: i64,
    tag_picker_is_batch: bool,
    // --- Cached selected count ---
    pub(crate) selected_count: usize,
    // --- Note editing state ---
    /// Which item is currently in note-edit mode (-1 = none).
    editing_note_id: i64,
    /// Shared InputState entity for the inline note editor.
    /// Created once at init, value is updated when editing starts.
    note_input: Entity<InputState>,
    /// Shared InputState for the tag picker's create-tag input.
    tag_create_input: Entity<InputState>,
    /// Per-list image cache so card thumbnails/icons can be released on hide.
    image_cache: Entity<RetainAllImageCache>,
    /// Active confirmation dialog (None = hidden).
    confirm_dialog: Option<ConfirmDialogState>,
    /// Persisted last-selected item ID — survives across set_items calls
    /// (including the empty-item clear during window hide).
    last_selected_id: i64,
    theme: ClippiTheme,
    last_lang_version: u64,
    /// WindowManager entity for hotkey recording coordination.
    window_manager: Entity<crate::ui::window_manager::WindowManager>,
    /// ID of item currently in hotkey-recording mode (-1 = none).
    recording_hotkey_id: i64,
    /// Serialized paste format selected while recording an item hotkey.
    recording_hotkey_format: String,
    /// One-shot guard for an ESC event already consumed by inline UI.
    escape_consumed_inline: bool,
    /// Candidate for Ctrl/Cmd/Shift double-click paste (see `handle_card_click`).
    modified_double_click_candidate: Option<ModifiedDoubleClickCandidate>,
}

impl ClipboardListView {
    pub fn new(
        items: Vec<ClipboardItem>,
        state: Entity<AppState>,
        theme: ClippiTheme,
        window: &mut Window,
        cx: &mut App,
        window_manager: Entity<crate::ui::window_manager::WindowManager>,
    ) -> Self {
        let card_height_mode = state.read(cx).settings.card_height_mode.clone();
        let item_sizes = Rc::new(Self::compute_sizes(&items, &card_height_mode));
        Self {
            items,
            item_sizes,
            card_height_mode,
            scroll_handle: VirtualListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            selected_ids: Vec::new(),
            selected_index: None,
            anchor_index: None,
            state,
            search_bar: None,
            hovered_index: None,
            last_hover_pos: None,
            context_menu_visible: false,
            context_menu_x: 0.0,
            context_menu_y: 0.0,
            context_menu_item: None,
            context_menu_is_batch: false,
            tag_picker_visible: false,
            tag_picker_x: 0.0,
            tag_picker_y: 0.0,
            tag_picker_item_id: -1,
            tag_picker_is_batch: false,
            selected_count: 0,
            editing_note_id: -1,
            note_input: cx.new(|cx| {
                InputState::new(window, cx).placeholder(I18nKey::ListNotePlaceholder.text())
            }),
            tag_create_input: cx.new(|cx| {
                InputState::new(window, cx).placeholder(I18nKey::TagCreatePlaceholder.text())
            }),
            image_cache: RetainAllImageCache::new(cx),
            confirm_dialog: None,
            last_selected_id: -1,
            theme,
            last_lang_version: crate::core::i18n::lang_version(),
            window_manager,
            recording_hotkey_id: -1,
            recording_hotkey_format: String::new(),
            escape_consumed_inline: false,
            modified_double_click_candidate: None,
        }
    }

    pub fn set_items(&mut self, items: Vec<ClipboardItem>, cx: &mut Context<Self>) {
        self.state
            .update(cx, |state, _| state.clear_usage_sync_request());
        self.item_sizes = Rc::new(Self::compute_sizes(&items, &self.card_height_mode));
        // --- Persist current selection before swap (survives empty-item clears ---
        // --- during window hide, when hide() emits ClipboardChanged).         ---
        if let Some(idx) = self.selected_index {
            if let Some(item) = self.items.get(idx) {
                self.last_selected_id = item.id;
            }
        }
        self.items = items;
        self.selected_ids.clear();
        self.selected_index = None;
        self.anchor_index = None;
        self.selected_count = 0;
        self.hovered_index = None;
        self.modified_double_click_candidate = None;
        let scroll_to_latest = self.state.read(cx).settings.auto_scroll_to_top;
        if scroll_to_latest && !self.items.is_empty() {
            let latest_idx = self
                .items
                .iter()
                .enumerate()
                .max_by_key(|(_, item)| &item.updated_at)
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.select_index_without_scroll(latest_idx, cx);
            self.scroll_handle
                .scroll_to_item(latest_idx, ScrollStrategy::Top);
        } else if self.last_selected_id > 0 {
            // --- Keep position: restore the previously selected item by persisted ID ---
            if let Some(idx) = self
                .items
                .iter()
                .position(|item| item.id == self.last_selected_id)
            {
                self.select_index_without_scroll(idx, cx);
                self.scroll_handle.scroll_to_item(idx, ScrollStrategy::Top);
            }
        }
        // --- Fallback: select first item when nothing else matched ---
        // --- (first launch, persisted item deleted, empty history, etc.) ---
        if self.selected_index.is_none() && !self.items.is_empty() {
            self.select_index_without_scroll(0, cx);
        }
        cx.notify();
    }

    /// Whether the current search applies favorites-first reordering.
    fn favorites_first_active(&self, cx: &Context<Self>) -> bool {
        let app = self.state.read(cx);
        app.settings.search_favorites_first && app.filters.has_keyword()
    }

    /// Explicit event: a favorites-first search was re-typed or batch-toggled.
    /// Bring the top of the reordered result into view and make the first
    /// result the active item. Only called from user-intent paths (search box
    /// input, batch favorite toggles) — generic refreshes must not jump.
    pub(crate) fn select_and_scroll_to_top_if_favorites_first(&mut self, cx: &mut Context<Self>) {
        if self.favorites_first_active(cx) && !self.items.is_empty() {
            // The hover toolbar is index-based; the reorder invalidates it, so
            // drop it and let the next mouse move recompute the hovered row.
            self.hovered_index = None;
            self.select_index_without_scroll(0, cx);
            self.scroll_handle.scroll_to_item(0, ScrollStrategy::Top);
        }
    }

    /// Explicit event: a single favorite toggle reordered the result.
    /// Keep the toggled item as the active target — reselect it by id and
    /// bring it into view at its new group position, so a following Enter or
    /// copy acts on the same item the user just toggled. Falls back to the
    /// top of the list if the item left the result set.
    fn reselect_toggled_item_if_favorites_first(&mut self, item_id: i64, cx: &mut Context<Self>) {
        if !self.favorites_first_active(cx) || self.items.is_empty() {
            return;
        }
        // The hover toolbar is index-based; the reorder invalidates it.
        self.hovered_index = None;
        if let Some(idx) = self.items.iter().position(|item| item.id == item_id) {
            self.select_index_without_scroll(idx, cx);
            self.scroll_handle.scroll_to_item(idx, ScrollStrategy::Top);
        } else {
            self.select_and_scroll_to_top_if_favorites_first(cx);
        }
    }

    /// Drop the list's own item copies while the main window is hidden.
    ///
    /// AppState also clears its item list, but ClipboardListView keeps a
    /// separate Vec for virtual scrolling. Without clearing both, hidden
    /// windows can retain large clipboard payloads until the next reload.
    pub fn release_items_for_hide(&mut self, cx: &mut Context<Self>) {
        if let Some(idx) = self.selected_index {
            if let Some(item) = self.items.get(idx) {
                self.last_selected_id = item.id;
            }
        }

        self.items.clear();
        self.items.shrink_to_fit();
        self.item_sizes = Rc::new(Vec::new());
        self.selected_ids.clear();
        self.selected_ids.shrink_to_fit();
        self.selected_index = None;
        self.anchor_index = None;
        self.selected_count = 0;
        self.hovered_index = None;
        self.modified_double_click_candidate = None;
        self.context_menu_visible = false;
        self.context_menu_item = None;
        self.tag_picker_visible = false;
        self.tag_picker_item_id = -1;
        self.confirm_dialog = None;
        self.editing_note_id = -1;
        self.escape_consumed_inline = false;
        self.image_cache = RetainAllImageCache::new(cx);
        cx.notify();
    }

    /// Reload local items from AppState without resetting UI state.
    /// Use after mutations (toggle_favorite, tag ops, delete) to keep the list
    /// in sync. Items retain their current order; re-sort on next window open.
    pub(crate) fn sync_items_from_state(&mut self, cx: &mut Context<Self>) {
        self.state
            .update(cx, |state, _| state.clear_usage_sync_request());
        let app_items = self.state.read(cx).visible_items();
        self.item_sizes = Rc::new(Self::compute_sizes(&app_items, &self.card_height_mode));
        self.items = app_items;
        cx.notify();
    }

    /// Incremental sync after usage operations (copy/paste): only the touched
    /// items' `updated_at` is refreshed and the list reordered, keeping card
    /// heights and item payloads untouched. Falls back to one full sync when
    /// incremental syncing cannot be proven safe (transfer view, ID set
    /// mismatch, or no usage update happened).
    pub(crate) fn sync_items_from_state_for_usage(&mut self, cx: &mut Context<Self>) {
        let (touched, force_full_reload) = self
            .state
            .update(cx, |state, _| state.take_usage_sync_request());
        let selected_id = item_id_at(&self.items, self.selected_index);
        let anchor_id = item_id_at(&self.items, self.anchor_index);
        let hovered_id = item_id_at(&self.items, self.hovered_index);
        let safe = {
            let app = self.state.read(cx);
            can_incrementally_sync_usage(
                &self.items,
                self.item_sizes.len(),
                &app.items,
                &touched,
                app.transfer_filter_active,
                force_full_reload,
            )
        };
        if !safe {
            // One-time fallback: cannot prove the incremental update safe.
            self.sync_items_from_state(cx);
            self.selected_index = item_index(&self.items, selected_id).or_else(|| {
                self.selected_index
                    .filter(|index| *index < self.items.len())
            });
            self.anchor_index = item_index(&self.items, anchor_id)
                .or_else(|| self.anchor_index.filter(|index| *index < self.items.len()));
            self.hovered_index = item_index(&self.items, hovered_id);
            cx.notify();
        } else {
            let (sort_by_created, favorites_first) = {
                let app = self.state.read(cx);
                for id in &touched {
                    if let Some(remote) = app.items.iter().find(|item| item.id == *id) {
                        if let Some(local) = self.items.iter_mut().find(|item| item.id == *id) {
                            local.updated_at = remote.updated_at;
                        }
                    }
                }
                (
                    app.settings.sort_by_created,
                    app.settings.search_favorites_first
                        && app.filters.has_keyword()
                        && !app.filters.is_favorites_active(),
                )
            };
            // Pairwise reorder: move cached card heights together with their
            // items (item payloads are unchanged, so heights stay valid).
            let sizes: Vec<gpui::Size<gpui::Pixels>> = self.item_sizes.as_ref().clone();
            let mut pairs: Vec<(ClipboardItem, gpui::Size<gpui::Pixels>)> =
                std::mem::take(&mut self.items)
                    .into_iter()
                    .zip(sizes)
                    .collect();
            reorder_usage_pairs(&mut pairs, sort_by_created, favorites_first);
            let mut new_items = Vec::with_capacity(pairs.len());
            let mut new_sizes = Vec::with_capacity(pairs.len());
            for (item, size) in pairs {
                new_items.push(item);
                new_sizes.push(size);
            }
            self.items = new_items;
            self.item_sizes = Rc::new(new_sizes);
            self.selected_index = item_index(&self.items, selected_id);
            self.anchor_index = item_index(&self.items, anchor_id);
            self.hovered_index = item_index(&self.items, hovered_id);
            cx.notify();
        }
        if self.state.read(cx).settings.auto_scroll_to_top && !self.items.is_empty() {
            let latest_idx = self
                .items
                .iter()
                .enumerate()
                .max_by_key(|(_, item)| &item.updated_at)
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.select_index_without_scroll(latest_idx, cx);
            self.scroll_handle
                .scroll_to_item(latest_idx, ScrollStrategy::Top);
            cx.notify();
        }
    }

    pub(crate) fn refresh_settings_from_state(
        &mut self,
        scroll_to_top: bool,
        cx: &mut Context<Self>,
    ) {
        let card_height_mode = self.state.read(cx).settings.card_height_mode.clone();
        if self.card_height_mode != card_height_mode {
            self.card_height_mode = card_height_mode;
            self.item_sizes = Rc::new(Self::compute_sizes(&self.items, &self.card_height_mode));
        }
        if scroll_to_top && !self.items.is_empty() {
            let latest_idx = self
                .items
                .iter()
                .enumerate()
                .max_by_key(|(_, item)| &item.updated_at)
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.select_index_without_scroll(latest_idx, cx);
            self.scroll_handle
                .scroll_to_item(latest_idx, ScrollStrategy::Top);
        }
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window) {
        self.focus_handle.focus(window);
    }

    fn select_index(
        &mut self,
        index: usize,
        scroll_strategy: ScrollStrategy,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.items.get(index) else {
            return;
        };
        let item_id = item.id;
        self.selected_ids.clear();
        self.selected_ids.push(item_id);
        self.selected_index = Some(index);
        self.anchor_index = Some(index);
        self.selected_count = 1;
        self.modified_double_click_candidate = None;
        self.scroll_handle.scroll_to_item(index, scroll_strategy);
        self.state.update(cx, move |state, _cx| {
            state.select_single(item_id);
        });
        cx.notify();
    }

    fn select_index_without_scroll(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(index) else {
            return;
        };
        let item_id = item.id;
        self.selected_ids.clear();
        self.selected_ids.push(item_id);
        self.selected_index = Some(index);
        self.anchor_index = Some(index);
        self.selected_count = 1;
        self.modified_double_click_candidate = None;
        self.state.update(cx, move |state, _cx| {
            state.select_single(item_id);
        });
        cx.notify();
    }

    /// Toggle selection of an item (Ctrl+click).
    fn toggle_index(&mut self, index: usize, cx: &mut Context<Self>) {
        self.modified_double_click_candidate = None;
        let Some(item) = self.items.get(index) else {
            return;
        };
        let item_id = item.id;
        if let Some(pos) = self.selected_ids.iter().position(|&x| x == item_id) {
            // --- Don't deselect the last remaining item — always keep at least one selected ---
            if self.selected_ids.len() <= 1 {
                return;
            }
            self.selected_ids.remove(pos);
            // --- Keep selected_index/anchor_index pointing at a remaining selected item ---
            if self.selected_index == Some(index) || self.anchor_index == Some(index) {
                let first_remaining = self
                    .selected_ids
                    .first()
                    .and_then(|&id| self.items.iter().position(|item| item.id == id));
                if self.selected_index == Some(index) {
                    self.selected_index = first_remaining;
                }
                if self.anchor_index == Some(index) {
                    self.anchor_index = first_remaining;
                }
            }
        } else {
            self.selected_ids.push(item_id);
            self.anchor_index = Some(index);
            self.selected_index = Some(index);
        }
        self.selected_count = self.selected_ids.len();
        let selected = self.selected_ids.clone();
        self.state.update(cx, move |state, _cx| {
            state.range_select(&selected);
        });
        cx.notify();
    }

    /// Range select from anchor to given index (Shift+click).
    fn range_select_to_index(&mut self, index: usize, cx: &mut Context<Self>) {
        self.modified_double_click_candidate = None;
        let anchor = match self.anchor_index {
            Some(a) => a,
            None => {
                // --- No anchor — fall back to single select ---
                self.select_index_without_scroll(index, cx);
                return;
            }
        };
        self.selected_ids.clear();
        if anchor <= index {
            // --- Selecting downward: anchor (top) gets #1, count goes down ---
            for i in anchor..=index {
                if let Some(item) = self.items.get(i) {
                    self.selected_ids.push(item.id);
                }
            }
        } else {
            // --- Selecting upward: anchor (bottom) gets #1, count goes up ---
            for i in (index..=anchor).rev() {
                if let Some(item) = self.items.get(i) {
                    self.selected_ids.push(item.id);
                }
            }
        }
        self.selected_index = Some(index);
        self.selected_count = self.selected_ids.len();
        let selected = self.selected_ids.clone();
        self.state.update(cx, move |state, _cx| {
            state.range_select(&selected);
        });
        cx.notify();
    }

    pub(crate) fn select_next(&mut self, scroll_strategy: ScrollStrategy, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        let next_index = self
            .selected_index
            .map(|index| (index + 1).min(self.items.len().saturating_sub(1)))
            .unwrap_or(0);
        self.select_index(next_index, scroll_strategy, cx);
    }

    pub(crate) fn select_previous(
        &mut self,
        scroll_strategy: ScrollStrategy,
        cx: &mut Context<Self>,
    ) {
        if self.items.is_empty() {
            return;
        }
        let previous_index = self
            .selected_index
            .map(|index| index.saturating_sub(1))
            .unwrap_or(0);
        self.select_index(previous_index, scroll_strategy, cx);
    }

    // --- Keyboard-shortcut action helpers (pub(crate) so SearchBar can call them) ---

    /// Dismiss floating panels instead of pasting when any panel is open.
    /// Shared by `action_paste` and `action_paste_bitmap_or_default` so the
    /// list key handler and the search bar behave identically.
    fn guard_panels_before_paste(&mut self, cx: &mut Context<Self>) -> bool {
        if self.has_any_panel_or_editing() {
            self.dismiss_all_panels(cx);
            true
        } else {
            false
        }
    }

    /// ID of the single selected item, if exactly one item is selected.
    fn single_selected_id(&self) -> Option<i64> {
        if self.selected_ids.len() == 1 {
            self.selected_ids.first().copied()
        } else {
            None
        }
    }

    /// Paste selected item(s). Called from list key handler and search bar.
    pub(crate) fn action_paste(&mut self, plain: bool, cx: &mut Context<Self>) {
        if self.guard_panels_before_paste(cx) {
            return;
        }
        if self.selected_ids.len() > 1 {
            let ids = self.selected_ids.clone();
            self.state.update(cx, |s, _cx| s.batch_paste(&ids, plain));
        } else if let Some(item) = self
            .single_selected_id()
            .and_then(|id| self.items.iter().find(|item| item.id == id))
        {
            let id = item.id;
            if plain {
                self.state.update(cx, |s, _cx| s.paste_item_plain(id));
            } else {
                self.state.update(cx, |s, _cx| s.paste_item(id, plain));
            }
        }
        // Incremental when a usage update happened; falls back to a full sync
        // when nothing was touched (e.g. the selection no longer exists).
        self.sync_items_from_state_for_usage(cx);
    }

    /// Ctrl/Cmd+Enter: paste the single selected image as bitmap; any other
    /// selection falls back to the default paste behavior (`action_paste`).
    pub(crate) fn action_paste_bitmap_or_default(&mut self, cx: &mut Context<Self>) {
        if self.guard_panels_before_paste(cx) {
            return;
        }
        if let Some(item) = self
            .single_selected_id()
            .and_then(|id| self.items.iter().find(|item| item.id == id))
        {
            if item.content_type == ContentType::Image {
                let id = item.id;
                self.state.update(cx, |s, _cx| s.paste_image_as_bitmap(id));
                self.sync_items_from_state_for_usage(cx);
                return;
            }
        }
        let plain = self.state.read(cx).settings.copy_as_plain_text;
        self.action_paste(plain, cx);
    }

    /// Single-click selection plus modified double-click candidate arbitration.
    ///
    /// A candidate is established only when the first modified click targets
    /// the single selected item and leaves the selection set unchanged; the
    /// additive+Shift combination never forms a candidate.
    fn handle_card_click(
        &mut self,
        index: usize,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // --- Clicking another card while editing → commit first ---
        if self.editing_note_id > 0 {
            self.commit_note_edit(window, cx);
        }

        let before_ids = self.selected_ids.clone();
        let was_unique_selected = self.selected_ids.len() == 1
            && self
                .items
                .get(index)
                .is_some_and(|item| self.selected_ids[0] == item.id);
        let candidate_modifier = modified_double_click_kind(modifiers);

        if additive_selection_modifier(modifiers) {
            self.toggle_index(index, cx);
        } else if modifiers.shift {
            self.range_select_to_index(index, cx);
        } else {
            self.select_index_without_scroll(index, cx);
        }

        self.modified_double_click_candidate = match candidate_modifier {
            Some(modifier) if was_unique_selected && self.selected_ids == before_ids => self
                .items
                .get(index)
                .map(|item| ModifiedDoubleClickCandidate {
                    item_id: item.id,
                    modifier,
                }),
            _ => None,
        };
    }

    /// Default double-click behavior (no modifiers): paste with full formatting.
    fn double_click_paste_default(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.selected_ids.len() > 1 {
            let ids = self.selected_ids.clone();
            self.state.update(cx, |s, _cx| s.batch_paste(&ids, false));
            self.sync_items_from_state_for_usage(cx);
        } else if let Some(item) = self.items.get(index) {
            if item.id < 0 && item.meta_type == "transfer" {
                let file_data = crate::core::types::FileData::from_json(&item.file_data);
                let hash = file_data.remote_hash;
                let local_path = file_data.files.first().map(|file| file.path.clone());
                self.state.update(cx, |state, _cx| {
                    if let Some(path) = local_path.filter(|path| !path.is_empty()) {
                        state.open_transfer_location(&path);
                    } else {
                        state.download_transfer_entry(&hash);
                    }
                });
                self.sync_items_from_state(cx);
                return;
            }
            let item_id = item.id;
            self.state.update(cx, |s, _cx| s.paste_item(item_id, false));
            self.sync_items_from_state_for_usage(cx);
        }
    }

    /// Double-click with modifiers: paste only when the first click left the
    /// selection unchanged (candidate established); otherwise consume the click
    /// and keep the first click's selection result.
    fn handle_double_click(&mut self, index: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        match modified_double_click_kind(modifiers) {
            // No modifiers at all — existing default paste, never gated by candidate.
            None if modifiers.number_of_modifiers() == 0 => {
                self.double_click_paste_default(index, cx);
                self.modified_double_click_candidate = None;
            }
            // Bare primary modifier (Ctrl/Cmd) — candidate-gated bitmap paste.
            Some(ModifiedPasteModifier::Bitmap) => {
                let Some(candidate) = self.modified_double_click_candidate.take() else {
                    return;
                };
                let Some(item) = self.items.get(index) else {
                    return;
                };
                if candidate.item_id == item.id
                    && matches!(candidate.modifier, ModifiedPasteModifier::Bitmap)
                    && item.content_type == ContentType::Image
                {
                    let id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_image_as_bitmap(id));
                    self.sync_items_from_state_for_usage(cx);
                }
            }
            // Bare Shift — candidate-gated plain-text paste.
            Some(ModifiedPasteModifier::PlainText) => {
                let Some(candidate) = self.modified_double_click_candidate.take() else {
                    return;
                };
                let Some(item) = self.items.get(index) else {
                    return;
                };
                if candidate.item_id == item.id
                    && matches!(candidate.modifier, ModifiedPasteModifier::PlainText)
                {
                    let id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_item_plain(id));
                    self.sync_items_from_state_for_usage(cx);
                }
            }
            // Any other modifier combination — consume the click, never paste.
            None => {
                self.modified_double_click_candidate = None;
            }
        }
    }

    /// Toggle favorite on selected item(s).
    pub(crate) fn action_toggle_favorite(&mut self, cx: &mut Context<Self>) {
        let mut toggled_id: Option<i64> = None;
        if self.selected_count > 1 {
            self.state.update(cx, |s, _cx| s.batch_toggle_favorite());
        } else if let Some(idx) = self.selected_index {
            if let Some(item) = self.items.get(idx) {
                let id = item.id;
                toggled_id = Some(id);
                self.state.update(cx, |s, _cx| s.toggle_favorite(id));
            }
        }
        self.sync_items_from_state(cx);
        match toggled_id {
            Some(id) => self.reselect_toggled_item_if_favorites_first(id, cx),
            None => self.select_and_scroll_to_top_if_favorites_first(cx),
        }
    }

    /// Open the edit panel for the selected item.
    pub(crate) fn action_edit(&mut self, cx: &mut Context<Self>) {
        if let Some(idx) = self.selected_index {
            if let Some(item) = self.items.get(idx) {
                cx.emit(ClipboardListEvent::OpenEdit(item.id));
            }
        }
    }

    /// Start inline note editing for the selected item.
    pub(crate) fn action_edit_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(idx) = self.selected_index {
            if let Some(item) = self.items.get(idx) {
                if item.id != self.editing_note_id {
                    let id = item.id;
                    let note = item.note.clone();
                    self.start_note_edit(id, &note, window, cx);
                }
            }
        }
    }

    /// Open the tag picker from keyboard actions, anchored to the selected row.
    pub(crate) fn action_show_tag_picker(&mut self, cx: &mut Context<Self>) {
        if self.has_any_panel_or_editing() {
            return;
        }

        let Some(selected_index) = self.selected_index else {
            return;
        };

        let Some(item) = self.items.get(selected_index) else {
            return;
        };

        let y = self
            .selected_item_tag_picker_y(selected_index)
            .unwrap_or(TAG_PICKER_FALLBACK_Y);

        let is_batch = self.selected_count > 1;
        let item_id = if is_batch { -1 } else { item.id };
        self.open_tag_picker(is_batch, item_id, y, cx);
    }

    fn open_tag_picker(
        &mut self,
        is_batch: bool,
        item_id: i64,
        anchor_y: f32,
        cx: &mut Context<Self>,
    ) {
        self.tag_picker_visible = true;
        self.tag_picker_x = self.fixed_tag_picker_x();
        self.tag_picker_y = anchor_y;
        self.tag_picker_is_batch = is_batch;
        self.tag_picker_item_id = if is_batch { -1 } else { item_id };
        cx.notify();
    }

    fn fixed_tag_picker_x(&self) -> f32 {
        let list_bounds = self.scroll_handle.bounds();
        if list_bounds.size.width <= px(0.) {
            return TAG_PICKER_FALLBACK_X;
        }

        f32::from(list_bounds.left()) + (f32::from(list_bounds.size.width) - TAG_PICKER_WIDTH) / 2.0
    }

    fn selected_item_tag_picker_y(&self, selected_index: usize) -> Option<f32> {
        let list_bounds = self.scroll_handle.bounds();
        if list_bounds.size.width <= px(0.) || list_bounds.size.height <= px(0.) {
            return None;
        }

        let row_top = self
            .item_sizes
            .iter()
            .take(selected_index)
            .map(|size| f32::from(size.height))
            .sum::<f32>();
        let scroll_y = f32::from(self.scroll_handle.offset().y);
        let y = f32::from(list_bounds.top()) + row_top + scroll_y + TAG_PICKER_CARD_ANCHOR_Y;

        Some(y)
    }

    fn pointer_tag_picker_y(&self) -> f32 {
        self.last_hover_pos
            .map(|p| f32::from(p.y))
            .unwrap_or(TAG_PICKER_FALLBACK_Y)
    }

    fn selected_transfer_hashes(&self) -> Vec<String> {
        self.selected_ids
            .iter()
            .filter_map(|selected_id| {
                let item = self.items.iter().find(|item| item.id == *selected_id)?;
                let file_data = crate::core::types::FileData::from_json(&item.file_data);
                (item.id < 0 && item.meta_type == "transfer" && !file_data.remote_hash.is_empty())
                    .then_some(file_data.remote_hash)
            })
            .collect()
    }

    fn show_transfer_batch_delete_confirm(&mut self, cx: &mut Context<Self>) {
        let hashes = self.selected_transfer_hashes();
        if hashes.is_empty() {
            return;
        }
        self.confirm_dialog = Some(ConfirmDialogState::TransferBatch { hashes });
        cx.notify();
    }

    /// Show the delete confirmation dialog for selected item(s).
    pub(crate) fn action_delete(&mut self, cx: &mut Context<Self>) {
        let transfer_hashes = self.selected_transfer_hashes();
        if !transfer_hashes.is_empty() && transfer_hashes.len() == self.selected_ids.len() {
            if transfer_hashes.len() == 1 {
                self.confirm_dialog = Some(ConfirmDialogState::Transfer {
                    hash: transfer_hashes[0].clone(),
                });
            } else {
                self.confirm_dialog = Some(ConfirmDialogState::TransferBatch {
                    hashes: transfer_hashes,
                });
            }
        } else if self.selected_count > 1 {
            let count = self.selected_ids.len();
            self.confirm_dialog = Some(ConfirmDialogState::Batch { count });
        } else if let Some(idx) = self.selected_index {
            if let Some(item) = self.items.get(idx) {
                self.confirm_dialog = Some(ConfirmDialogState::Single { id: item.id });
            }
        }
        cx.notify();
    }

    /// Reduce multi-selection to single selection, keeping only the first selected item.
    /// Returns true if the selection was reduced, false if it was already single/none.
    pub fn reduce_to_single_selection(&mut self, cx: &mut Context<Self>) -> bool {
        if self.selected_count <= 1 {
            return false;
        }
        let first_id = self.selected_ids[0];
        if let Some(idx) = self.items.iter().position(|item| item.id == first_id) {
            self.selected_ids.clear();
            self.selected_ids.push(first_id);
            self.selected_index = Some(idx);
            self.anchor_index = Some(idx);
            self.selected_count = 1;
            self.modified_double_click_candidate = None;
            self.state.update(cx, |state, _cx| {
                state.select_single(first_id);
            });
            cx.notify();
            return true;
        }
        false
    }

    /// Handle ESC key: dismiss panels/editing → reduce multi-select → signal close.
    /// Returns `true` if the event was consumed (panels dismissed or selection reduced),
    /// `false` if the window should close.
    fn handle_escape_inner(
        &mut self,
        mark_inline_bubble_guard: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.escape_consumed_inline {
            self.escape_consumed_inline = false;
            return true;
        }
        if self.recording_hotkey_id > 0 {
            self.cancel_hotkey_recording(cx);
            self.escape_consumed_inline = mark_inline_bubble_guard;
            return true;
        }
        if self.has_any_panel_or_editing() {
            self.dismiss_all_panels(cx);
            self.editing_note_id = -1;
            self.confirm_dialog = None;
            self.escape_consumed_inline = mark_inline_bubble_guard;
            cx.notify();
            return true;
        }
        self.reduce_to_single_selection(cx)
    }

    pub fn handle_escape(&mut self, cx: &mut Context<Self>) -> bool {
        self.handle_escape_inner(true, cx)
    }

    pub fn handle_escape_from_root(&mut self, cx: &mut Context<Self>) -> bool {
        self.handle_escape_inner(false, cx)
    }

    // --- Context menu state accessors ---

    pub fn context_menu_visible(&self) -> bool {
        self.context_menu_visible
    }

    pub fn context_menu_is_batch(&self) -> bool {
        self.context_menu_is_batch
    }

    pub fn context_menu_is_transfer_batch(&self) -> bool {
        self.context_menu_is_batch
            && !self.selected_ids.is_empty()
            && self.selected_transfer_hashes().len() == self.selected_ids.len()
    }

    pub fn can_merge_selected_items(&self, cx: &App) -> bool {
        self.state.read(cx).can_merge_selected_items()
    }

    pub fn context_menu_position(&self) -> (f32, f32) {
        (self.context_menu_x, self.context_menu_y)
    }

    pub fn context_menu_item(&self) -> Option<&ClipboardItem> {
        self.context_menu_item.as_ref()
    }

    pub fn dismiss_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu_visible = false;
        self.context_menu_item = None;
        self.hovered_index = None;
        cx.notify();
    }

    /// Dismiss all floating panels (context menu, tag picker).
    pub fn dismiss_all_panels(&mut self, cx: &mut Context<Self>) {
        self.context_menu_visible = false;
        self.context_menu_item = None;
        self.hovered_index = None;
        self.tag_picker_visible = false;
        self.tag_picker_item_id = -1;
        cx.notify();
    }

    /// Check whether any floating panel or inline editing is active.
    /// When true, Enter key should NOT trigger paste.
    fn has_any_panel_or_editing(&self) -> bool {
        self.context_menu_visible
            || self.tag_picker_visible
            || self.confirm_dialog.is_some()
            || self.editing_note_id > 0
            || self.recording_hotkey_id > 0
    }

    pub fn tag_picker_visible(&self) -> bool {
        self.tag_picker_visible
    }

    pub fn tag_picker_position(&self) -> (f32, f32) {
        (self.tag_picker_x, self.tag_picker_y)
    }

    pub fn tag_picker_is_batch(&self) -> bool {
        self.tag_picker_is_batch
    }

    pub fn tag_create_input(&self) -> &Entity<InputState> {
        &self.tag_create_input
    }

    pub fn hide_tag_picker(&mut self, cx: &mut Context<Self>) {
        self.tag_picker_visible = false;
        self.tag_picker_item_id = -1;
        cx.notify();
    }

    fn show_context_menu_tag_picker(&mut self, cx: &mut Context<Self>) {
        let item_id = self.context_menu_item.as_ref().map_or(-1, |item| item.id);
        self.open_tag_picker(self.context_menu_is_batch, item_id, self.context_menu_y, cx);
    }

    fn show_hover_tag_picker(&mut self, is_batch: bool, cx: &mut Context<Self>) {
        let item_id = if is_batch {
            -1
        } else {
            let Some(index) = self.hovered_index else {
                return;
            };
            self.items.get(index).map_or(-1, |item| item.id)
        };
        if !is_batch && item_id <= 0 {
            return;
        }

        self.open_tag_picker(is_batch, item_id, self.pointer_tag_picker_y(), cx);
    }

    pub fn tag_picker_rows(
        &self,
        cx: &mut Context<Self>,
    ) -> Vec<(crate::core::types::TagInfo, TagState)> {
        let app_state = self.state.read(cx);
        app_state
            .tags
            .iter()
            .map(|tag| {
                let state = if self.tag_picker_is_batch {
                    let selected_items: Vec<&ClipboardItem> = self
                        .items
                        .iter()
                        .filter(|item| self.selected_ids.contains(&item.id))
                        .collect();
                    let selected_len = selected_items.len();
                    let tagged_count = selected_items
                        .iter()
                        .filter(|item| item.tags.iter().any(|item_tag| item_tag.id == tag.id))
                        .count();
                    if selected_len > 0 && tagged_count == selected_len {
                        TagState::All
                    } else if tagged_count > 0 {
                        TagState::Partial
                    } else {
                        TagState::None
                    }
                } else {
                    self.items
                        .iter()
                        .find(|item| item.id == self.tag_picker_item_id)
                        .filter(|item| item.tags.iter().any(|item_tag| item_tag.id == tag.id))
                        .map_or(TagState::None, |_| TagState::All)
                };
                (tag.clone(), state)
            })
            .collect()
    }

    pub fn toggle_picker_tag(&mut self, tag_id: i64, state: TagState, cx: &mut Context<Self>) {
        let target_id = self.tag_picker_item_id;
        if self.tag_picker_is_batch {
            let ids = self.selected_ids.clone();
            if state == TagState::None {
                self.state
                    .update(cx, |s, _cx| s.batch_add_tag(&ids, tag_id));
            } else {
                self.state
                    .update(cx, |s, _cx| s.batch_remove_tag(&ids, tag_id));
            }
        } else if target_id > 0 {
            self.state
                .update(cx, |s, _cx| s.toggle_item_tag(target_id, tag_id));
        }
        self.sync_items_from_state(cx);
    }

    pub fn create_tag_from_picker(&mut self, name: &str, cx: &mut Context<Self>) {
        self.state.update(cx, |s, _cx| s.create_tag(name));
        self.sync_items_from_state(cx);
    }

    pub fn clear_picker_tags(&mut self, cx: &mut Context<Self>) {
        let target_id = self.tag_picker_item_id;
        if self.tag_picker_is_batch {
            let ids = self.selected_ids.clone();
            self.state.update(cx, |s, _cx| s.clear_tags_for_items(&ids));
            self.hide_tag_picker(cx);
        } else if target_id > 0 {
            self.state.update(cx, |s, _cx| s.clear_item_tags(target_id));
        }
        self.sync_items_from_state(cx);
    }

    /// Keep an affected item selected and visible when an operation explicitly
    /// needs to relocate it. Tag and hotkey metadata updates do not use this
    /// helper because they must preserve the current scroll offset.
    fn scroll_to_item_if_visible(&mut self, item_id: i64, cx: &mut Context<Self>) {
        if item_id <= 0 {
            return;
        }
        if let Some(new_index) = self.items.iter().position(|item| item.id == item_id) {
            self.select_index_without_scroll(new_index, cx);
            self.scroll_handle
                .scroll_to_item(new_index, ScrollStrategy::Top);
        }
    }

    fn inline_locked_item_id(&self) -> Option<i64> {
        if self.editing_note_id > 0 {
            Some(self.editing_note_id)
        } else if self.recording_hotkey_id > 0 {
            Some(self.recording_hotkey_id)
        } else {
            None
        }
    }

    fn lock_selection_to_item(&mut self, item_id: i64, cx: &mut Context<Self>) {
        let Some(index) = self.items.iter().position(|item| item.id == item_id) else {
            return;
        };
        self.selected_ids.clear();
        self.selected_ids.push(item_id);
        self.selected_index = Some(index);
        self.anchor_index = Some(index);
        self.selected_count = 1;
        self.modified_double_click_candidate = None;
        self.hovered_index = Some(index);
        self.state.update(cx, |state, _cx| {
            state.select_single(item_id);
        });
    }

    /// Start editing the note for an item.
    /// `initial_text` — from item.note (hover toolbar) or "" (context menu).
    fn start_note_edit(
        &mut self,
        id: i64,
        initial_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editing_note_id = id;
        self.lock_selection_to_item(id, cx);
        let text = SharedString::from(initial_text.to_string());
        self.note_input.update(cx, move |input, cx| {
            input.set_value(text, window, cx);
            // --- Auto-focus so user can type immediately ---
            input.focus_handle(cx).focus(window);
        });
        cx.notify();
    }

    /// Start recording a custom hotkey for the given item.
    fn start_hotkey_recording(&mut self, id: i64, _window: &mut Window, cx: &mut Context<Self>) {
        self.context_menu_visible = false;
        self.context_menu_item = None;
        self.tag_picker_visible = false;
        self.tag_picker_item_id = -1;
        self.confirm_dialog = None;
        self.hovered_index = None;
        let format = self
            .items
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.custom_hotkey_format.clone())
            .filter(|format| !format.is_empty())
            .unwrap_or_else(|| {
                serde_json::to_string(&HotkeyPasteFormat::Default).unwrap_or_default()
            });
        self.recording_hotkey_format = format.clone();
        self.window_manager.update(cx, |wm, cx| {
            wm.start_item_hotkey_recording(id, format, cx);
        });
        self.recording_hotkey_id = id;
        self.lock_selection_to_item(id, cx);
        cx.notify();
    }

    /// Cancel hotkey recording and restore normal card view.
    fn cancel_hotkey_recording(&mut self, cx: &mut Context<Self>) {
        self.window_manager
            .update(cx, |wm, _cx| wm.cancel_custom_recording());
        self.recording_hotkey_id = -1;
        self.recording_hotkey_format.clear();
        self.escape_consumed_inline = true;
        self.hovered_index = None;
        cx.notify();
    }

    fn confirm_hotkey_recording(&mut self, cx: &mut Context<Self>) {
        self.window_manager
            .update(cx, |wm, _cx| wm.cancel_custom_recording());
        self.recording_hotkey_id = -1;
        self.recording_hotkey_format.clear();
        self.escape_consumed_inline = true;
        self.hovered_index = None;
        cx.notify();
    }

    fn update_recording_hotkey_format(
        &mut self,
        format: HotkeyPasteFormat,
        cx: &mut Context<Self>,
    ) {
        if self.recording_hotkey_id <= 0 {
            return;
        }
        let serialized = serde_json::to_string(&format).unwrap_or_default();
        self.recording_hotkey_format = serialized.clone();
        self.window_manager.update(cx, |wm, _cx| {
            wm.update_recording_item_hotkey_format(serialized);
        });
        cx.notify();
    }

    /// Called by card when user triggers commit/cancel in the recording UI.
    pub(crate) fn finish_hotkey_recording_ui(&mut self, cx: &mut Context<Self>) {
        self.recording_hotkey_id = -1;
        self.recording_hotkey_format.clear();
        self.hovered_index = None;
        cx.notify();
    }

    /// Commit the current note edit to DB and exit edit mode.
    fn commit_note_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_note_id > 0 {
            let id = self.editing_note_id;
            let text = self.note_input.read(cx).value().to_string();
            // --- Persist to DB + update AppState.items (single source of truth) ---
            let note_text = text.clone();
            self.state.update(cx, move |state, _cx| {
                state.update_note(id, &note_text);
            });
            // --- Re-sync from AppState to also recompute card heights ---
            // --- (note change may switch card between min-height and auto-height) ---
            self.sync_items_from_state(cx);
        }
        self.editing_note_id = -1;
        self.hovered_index = None;
        // --- Return focus to the list so keyboard navigation continues to work ---
        self.focus_handle.focus(window);
        cx.notify();
    }

    pub(crate) fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu_visible = false;
        self.context_menu_item = None;
        cx.notify();
    }

    pub fn set_theme(&mut self, theme: ClippiTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// Get the current confirmation dialog state, if any.
    pub fn confirm_dialog_state(&self) -> Option<&ConfirmDialogState> {
        self.confirm_dialog.as_ref()
    }

    /// Dismiss the active confirmation dialog.
    pub fn dismiss_confirm_dialog(&mut self, cx: &mut Context<Self>) {
        self.confirm_dialog = None;
        cx.notify();
    }

    pub(crate) fn handle_menu_action(
        &mut self,
        action: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let plain = self.state.read(cx).settings.copy_as_plain_text;
        match action {
            "copy" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.copy_item(item_id, plain));
                    self.sync_items_from_state_for_usage(cx);
                }
            }
            "paste" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_item(item_id, plain));
                    self.sync_items_from_state_for_usage(cx);
                }
            }
            "paste_plain" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_item_plain(item_id));
                    self.sync_items_from_state_for_usage(cx);
                }
            }
            "paste_as_rgb" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_as_rgb(item_id));
                    self.sync_items_from_state_for_usage(cx);
                }
            }
            "paste_as_hex" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_as_hex(item_id));
                    self.sync_items_from_state_for_usage(cx);
                }
            }
            "batch_paste" => {
                let ids = self.selected_ids.clone();
                self.state.update(cx, |s, _cx| s.batch_paste(&ids, plain));
                self.sync_items_from_state_for_usage(cx);
            }
            "merge_selected" => {
                let new_id = self.state.update(cx, |s, _cx| s.merge_selected_items());
                self.sync_items_from_state(cx);
                if let Some(id) = new_id {
                    self.scroll_to_item_if_visible(id, cx);
                }
            }
            "edit_note" => {
                if let Some(ref item) = self.context_menu_item {
                    self.start_note_edit(item.id, &item.note.clone(), _window, cx);
                }
            }
            "set_hotkey" => {
                if let Some(ref item) = self.context_menu_item {
                    self.start_hotkey_recording(item.id, _window, cx);
                }
            }
            "remove_hotkey" => {
                if let Some(ref item) = self.context_menu_item {
                    let id = item.id;
                    self.state.update(cx, |s, _cx| s.clear_item_hotkey(id));
                    self.window_manager.update(cx, |wm, _cx| {
                        wm.unregister_item_hotkey_if_set(id);
                    });
                    self.sync_items_from_state(cx);
                }
            }
            "toggle_favorite" => {
                if let Some(ref item) = self.context_menu_item {
                    let id = item.id;
                    self.state.update(cx, |s, _cx| s.toggle_favorite(id));
                    self.sync_items_from_state(cx);
                    self.reselect_toggled_item_if_favorites_first(id, cx);
                }
            }
            "delete" => {
                if let Some(ref item) = self.context_menu_item {
                    let id = item.id;
                    self.hide_context_menu(cx);
                    self.confirm_dialog = Some(ConfirmDialogState::Single { id });
                    return; // Don't call hide_context_menu again at end
                }
            }
            "batch_favorite" => {
                self.state.update(cx, |s, _cx| s.batch_toggle_favorite());
                self.sync_items_from_state(cx);
                self.select_and_scroll_to_top_if_favorites_first(cx);
            }
            "batch_delete" => {
                let count = self.selected_ids.len();
                self.confirm_dialog = Some(ConfirmDialogState::Batch { count });
                cx.notify();
            }
            "batch_delete_transfer" => {
                self.show_transfer_batch_delete_confirm(cx);
            }
            "open_image" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state
                        .update(cx, |s, _cx| s.open_original_image(item_id));
                }
            }
            "paste_image_path" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_image_path(item_id));
                    self.sync_items_from_state_for_usage(cx);
                }
            }
            "paste_file_path" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_file_path(item_id));
                    self.sync_items_from_state_for_usage(cx);
                }
            }
            "upload_to_transfer" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state
                        .update(cx, |s, _cx| s.upload_to_transfer_station(item_id));
                    self.sync_items_from_state(cx);
                }
            }
            "download_transfer" => {
                if let Some(ref item) = self.context_menu_item {
                    let fd = crate::core::types::FileData::from_json(&item.file_data);
                    let hash = fd.remote_hash.clone();
                    self.state
                        .update(cx, |s, _cx| s.download_transfer_entry(&hash));
                    self.sync_items_from_state(cx);
                }
            }
            "delete_transfer" => {
                if let Some(ref item) = self.context_menu_item {
                    let fd = crate::core::types::FileData::from_json(&item.file_data);
                    self.confirm_dialog = Some(ConfirmDialogState::Transfer {
                        hash: fd.remote_hash,
                    });
                    cx.notify();
                }
            }
            "toggle_transfer_pin" => {
                if let Some(ref item) = self.context_menu_item {
                    let fd = crate::core::types::FileData::from_json(&item.file_data);
                    let is_pinned = item.tags.iter().any(|tag| {
                        tag.uid == crate::core::transfer_types::TRANSFER_STATUS_PINNED_UID
                    });
                    self.state.update(cx, |s, _cx| {
                        s.set_transfer_entry_pinned(&fd.remote_hash, !is_pinned);
                    });
                    self.sync_items_from_state(cx);
                }
            }
            "open_transfer_location" => {
                if let Some(ref item) = self.context_menu_item {
                    let file_data = crate::core::types::FileData::from_json(&item.file_data);
                    if let Some(file) = file_data.files.first() {
                        let path = file.path.clone();
                        self.state
                            .update(cx, |state, _cx| state.open_transfer_location(&path));
                    }
                }
            }
            "paste_image_bitmap" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state
                        .update(cx, |s, _cx| s.paste_image_as_bitmap(item_id));
                    self.sync_items_from_state_for_usage(cx);
                }
            }
            "paste_ocr" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_ocr(item_id));
                    self.sync_items_from_state_for_usage(cx);
                }
            }
            "qr_detect" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.qr_detect(item_id));
                    self.sync_items_from_state(cx);
                }
            }
            "show_tag_picker" => {
                self.show_context_menu_tag_picker(cx);
            }
            // --- Edit panel migration is tracked separately. ---
            "edit" => {
                if let Some(ref item) = self.context_menu_item {
                    cx.emit(ClipboardListEvent::OpenEdit(item.id));
                }
            }
            "open_location" => {
                if let Some(ref item) = self.context_menu_item {
                    let id = item.id;
                    self.state.update(cx, |s, _cx| s.open_item_location(id));
                }
            }
            _ => {}
        }
        self.hide_context_menu(cx);
    }

    fn handle_toolbar_action(
        &mut self,
        action: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !action.starts_with("batch_") && action != "merge_selected" {
            let Some(index) = self.hovered_index else {
                return;
            };
            if self.items.get(index).is_none() {
                return;
            }
            self.select_index_without_scroll(index, cx);
        }

        let plain = self.state.read(cx).settings.copy_as_plain_text;
        match action {
            "download_transfer" => {
                if let Some(item) = self.hovered_index.and_then(|index| self.items.get(index)) {
                    let file_data = crate::core::types::FileData::from_json(&item.file_data);
                    let hash = file_data.remote_hash;
                    self.state
                        .update(cx, |state, _cx| state.download_transfer_entry(&hash));
                    self.sync_items_from_state(cx);
                }
            }
            "delete_transfer" => {
                if let Some(item) = self.hovered_index.and_then(|index| self.items.get(index)) {
                    let file_data = crate::core::types::FileData::from_json(&item.file_data);
                    self.confirm_dialog = Some(ConfirmDialogState::Transfer {
                        hash: file_data.remote_hash,
                    });
                    cx.notify();
                }
            }
            "open_transfer_location" => {
                if let Some(item) = self.hovered_index.and_then(|index| self.items.get(index)) {
                    let file_data = crate::core::types::FileData::from_json(&item.file_data);
                    if let Some(file) = file_data.files.first() {
                        let path = file.path.clone();
                        self.state
                            .update(cx, |state, _cx| state.open_transfer_location(&path));
                    }
                }
            }
            "toggle_transfer_pin" => {
                if let Some(item) = self.hovered_index.and_then(|index| self.items.get(index)) {
                    let file_data = crate::core::types::FileData::from_json(&item.file_data);
                    let is_pinned = item.tags.iter().any(|tag| {
                        tag.uid == crate::core::transfer_types::TRANSFER_STATUS_PINNED_UID
                    });
                    self.state.update(cx, |state, _cx| {
                        state.set_transfer_entry_pinned(&file_data.remote_hash, !is_pinned);
                    });
                    self.sync_items_from_state(cx);
                }
            }
            "copy" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        let item_id = item.id;
                        self.state.update(cx, |s, _cx| s.copy_item(item_id, plain));
                        self.sync_items_from_state_for_usage(cx);
                    }
                }
            }
            "paste_plain" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        let item_id = item.id;
                        self.state.update(cx, |s, _cx| s.paste_item_plain(item_id));
                        self.sync_items_from_state_for_usage(cx);
                    }
                }
            }
            // --- Batch toolbar actions ---
            "batch_paste" => {
                let ids = self.selected_ids.clone();
                self.state.update(cx, |s, _cx| s.batch_paste(&ids, plain));
                self.sync_items_from_state_for_usage(cx);
            }
            "merge_selected" => {
                let new_id = self.state.update(cx, |s, _cx| s.merge_selected_items());
                self.sync_items_from_state(cx);
                if let Some(id) = new_id {
                    self.scroll_to_item_if_visible(id, cx);
                }
            }
            "batch_tag" => {
                self.show_hover_tag_picker(true, cx);
            }
            "edit_note" => {
                if let Some(index) = self.hovered_index {
                    let (note_id, note_text) = match self.items.get(index) {
                        Some(item) => (item.id, item.note.clone()),
                        None => return,
                    };
                    // --- Hover toolbar: pre-fill existing note ---
                    self.start_note_edit(note_id, &note_text, _window, cx);
                }
            }
            "show_tag_picker" => {
                self.show_hover_tag_picker(false, cx);
            }
            "toggle_favorite" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        let id = item.id;
                        self.state.update(cx, |s, _cx| s.toggle_favorite(id));
                        self.sync_items_from_state(cx);
                        self.reselect_toggled_item_if_favorites_first(id, cx);
                    }
                }
            }
            "delete" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        self.confirm_dialog = Some(ConfirmDialogState::Single { id: item.id });
                        cx.notify();
                    }
                }
            }
            "batch_favorite" => {
                self.state.update(cx, |s, _cx| s.batch_toggle_favorite());
                self.sync_items_from_state(cx);
                self.select_and_scroll_to_top_if_favorites_first(cx);
            }
            "batch_delete" => {
                let count = self.selected_ids.len();
                self.confirm_dialog = Some(ConfirmDialogState::Batch { count });
                cx.notify();
            }
            "batch_delete_transfer" => {
                self.show_transfer_batch_delete_confirm(cx);
            }
            "paste_image_bitmap" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        // --- Double-check the type even though the toolbar only ---
                        // --- generates this action for image cards.             ---
                        if item.content_type == ContentType::Image {
                            let item_id = item.id;
                            self.state
                                .update(cx, |s, _cx| s.paste_image_as_bitmap(item_id));
                            self.sync_items_from_state_for_usage(cx);
                        }
                    }
                }
            }
            "open_image" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        self.state
                            .update(cx, |s, _cx| s.open_original_image(item.id));
                    }
                }
            }
            "qr_action" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        self.state.update(cx, |s, _cx| s.qr_action(item.id));
                    }
                }
            }
            "open_location" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        self.state
                            .update(cx, |s, _cx| s.open_item_location(item.id));
                    }
                }
            }
            // --- Edit panel migration is tracked separately. ---
            "edit" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        cx.emit(ClipboardListEvent::OpenEdit(item.id));
                    }
                }
            }
            _ => {}
        }
    }

    fn compute_sizes(items: &[ClipboardItem], mode: &str) -> Vec<Size<Pixels>> {
        let mut sizes: Vec<_> = items
            .iter()
            .map(|item| {
                let h = estimate_card_height(item, mode) + CLIPBOARD_ROW_VERTICAL_SPACE;
                size(px(308.), px(h))
            })
            .collect();
        // Append a sentinel spacer so the last real item can scroll fully into view.
        // The render callback's `filter_map` (via `this.items.get(i)`) returns None
        // for this out-of-bounds index, so nothing visible is painted — just empty
        // --- scrollable space. This avoids inflating the last card's visual height ---
        // --- when there are only a few items in the list. ---
        if !sizes.is_empty() {
            sizes.push(size(px(308.), px(CLIPBOARD_BOTTOM_SCROLL_INSET)));
        }
        sizes
    }
}

impl Render for ClipboardListView {
    #[allow(refining_impl_trait_reachable)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // 语言切换时刷新 InputState placeholder
        let current = crate::core::i18n::lang_version();
        if self.last_lang_version != current {
            self.last_lang_version = current;
            self.note_input.update(cx, |state, cx| {
                state.set_placeholder(I18nKey::ListNotePlaceholder.text(), window, cx);
            });
            self.tag_create_input.update(cx, |state, cx| {
                state.set_placeholder(I18nKey::TagCreatePlaceholder.text(), window, cx);
            });
        }

        let item_sizes = self.item_sizes.clone();
        let items_count = self.item_sizes.len();
        let view = cx.entity();
        let focus_handle = self.focus_handle.clone();
        let theme = self.theme.clone();
        let bg = theme.bg;
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;

        let empty_state = items_count == 0;

        // --- Empty state — render a lightweight static placeholder, completely ---
        // --- avoiding the virtual list subtree so GPUI releases its element cache. ---
        if empty_state {
            return div()
                .flex_1()
                .h_full()
                .w_full()
                .overflow_hidden()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .px(px(8.))
                .pt(px(4.))
                .pb(px(8.))
                .rounded_b(px(12.))
                .bg(bg)
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(text_2)
                        .child(I18nKey::ListNoItems.text()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(text_3)
                        .child(I18nKey::ListEmptyHint.text()),
                )
                .into_any_element();
        }

        // --- Normal state — virtual scrolling list ---
        let list_entity = view.clone();
        let image_cache = self.image_cache.clone();

        div()
            .flex_1()
            .overflow_hidden()
            .track_focus(&focus_handle)
            .on_key_down(
                window.listener_for(&view, |this, event: &KeyDownEvent, window, cx| {
                    let key = event.keystroke.key.as_str();
                    let ctrl = primary_modifier_pressed(event.keystroke.modifiers);
                    let shift = event.keystroke.modifiers.shift;

                    if this.recording_hotkey_id > 0 {
                        match key {
                            "escape" => {
                                this.cancel_hotkey_recording(cx);
                                cx.stop_propagation();
                                return;
                            }
                            "enter" => {
                                this.confirm_hotkey_recording(cx);
                                cx.stop_propagation();
                                return;
                            }
                            "up" | "down" | "pageup" | "pagedown" | "home" | "end" => {
                                cx.stop_propagation();
                                return;
                            }
                            _ => {}
                        }
                    }

                    // --- Ctrl+Key shortcuts ---
                    if ctrl {
                        match key {
                            "f" => {
                                // Ctrl+F / Cmd+F — focus search bar (always works)
                                if let Some(ref search_bar) = this.search_bar {
                                    search_bar.update(cx, |bar, cx| {
                                        bar.focus(window, cx);
                                    });
                                }
                                cx.stop_propagation();
                            }
                            "enter" if !shift => {
                                // Ctrl/Cmd+Enter — bitmap paste for a single selected
                                // image, default paste otherwise. The floating-panel
                                // guard lives inside the action method. Ctrl/Cmd+Shift+Enter
                                // is intentionally unhandled.
                                this.action_paste_bitmap_or_default(cx);
                                cx.stop_propagation();
                            }
                            "d" if !this.has_any_panel_or_editing() => {
                                // Ctrl+D — toggle favorite
                                this.action_toggle_favorite(cx);
                                cx.stop_propagation();
                            }
                            "e" if !this.has_any_panel_or_editing() => {
                                // Ctrl+E — open edit panel
                                this.action_edit(cx);
                                cx.stop_propagation();
                            }
                            "c" if !this.has_any_panel_or_editing() => {
                                // Ctrl+C — copy selected item to clipboard
                                if let Some(idx) = this.selected_index {
                                    if let Some(item) = this.items.get(idx) {
                                        let id = item.id;
                                        let plain = this.state.read(cx).settings.copy_as_plain_text;
                                        this.state.update(cx, |s, _cx| s.copy_item(id, plain));
                                        this.sync_items_from_state_for_usage(cx);
                                    }
                                }
                                cx.stop_propagation();
                            }
                            "t" if !this.has_any_panel_or_editing() => {
                                // Ctrl+T — show tag picker
                                this.action_show_tag_picker(cx);
                                cx.stop_propagation();
                            }
                            _ => {}
                        }
                        return;
                    }

                    // --- Shift+Enter — paste as plain text ---
                    if shift && key == "enter" {
                        // Floating-panel guard handled by `action_paste`.
                        this.action_paste(true, cx);
                        cx.stop_propagation();
                        return;
                    }

                    // --- Modifier-free keys ---
                    match key {
                        "up" => {
                            this.dismiss_all_panels(cx);
                            this.select_previous(ScrollStrategy::Top, cx);
                            cx.stop_propagation();
                        }
                        "down" => {
                            this.dismiss_all_panels(cx);
                            this.select_next(ScrollStrategy::Bottom, cx);
                            cx.stop_propagation();
                        }
                        "enter" => {
                            // Paste with the plain-text setting; floating panels
                            // are dismissed instead by `action_paste`.
                            let plain = this.state.read(cx).settings.copy_as_plain_text;
                            this.action_paste(plain, cx);
                            cx.stop_propagation();
                        }
                        "space" if !this.has_any_panel_or_editing() => {
                            // Space — smart open based on item type
                            if let Some(idx) = this.selected_index {
                                if let Some(item) = this.items.get(idx) {
                                    let item_id = item.id;
                                    if item.content_type == ContentType::Image {
                                        this.state.update(cx, |s, _cx| s.open_original_image(item_id));
                                    } else if item.meta_type == "link"
                                        || item.meta_type == "path"
                                        || item.content_type == ContentType::File
                                    {
                                        this.state.update(cx, |s, _cx| s.open_item_location(item_id));
                                    }
                                }
                            }
                            cx.stop_propagation();
                        }
                        "f2" if !this.has_any_panel_or_editing() => {
                            // F2 — add/edit note for selected item
                            this.action_edit_note(window, cx);
                            cx.stop_propagation();
                        }
                        "delete" | "backspace" if !this.has_any_panel_or_editing() => {
                            // Delete — remove selected item(s)
                            this.action_delete(cx);
                            cx.stop_propagation();
                        }
                        "escape" => {
                            // Escape — dismiss panels → reduce multi-select → hide window
                            if !this.handle_escape(cx) {
                                cx.emit(ClipboardListEvent::RequestHide);
                            }
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }),
            )
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded_b(px(12.))
            .bg(bg)
            .on_scroll_wheel({
                let list_for_scroll_lock = list_entity.clone();
                move |_ev, _window, cx| {
                    list_for_scroll_lock.update(cx, |this, cx| {
                        if this.inline_locked_item_id().is_some() {
                            cx.stop_propagation();
                        }
                    });
                }
            })
            .on_mouse_move({
                let list_for_clear = list_entity.clone();
                move |_ev, _window, cx| {
                    list_for_clear.update(cx, |this, cx| {
                        if this.inline_locked_item_id().is_some() {
                            return;
                        }
                        if this.hovered_index.is_some() {
                            this.hovered_index = None;
                            cx.notify();
                        }
                    });
                }
            })
            .pt(px(4.))
            .pb(px(8.))
            .px(px(8.))
            .when(empty_state, |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .h(px(200.))
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(text_2)
                                .child(I18nKey::ListNoItems.text()),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(text_3)
                                .child(I18nKey::ListEmptyHint.text()),
                        ),
                )
            })
            .when(!empty_state, |el| {
                el.relative()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child(
                        v_virtual_list(
                            view.clone(),
                            "clippi-clipboard-list",
                            item_sizes,
                            move |this, range, _window, cx| {
                                let selected_count = this.selected_count;
                                let can_merge_selection =
                                    this.state.read(cx).can_merge_selected_items();
                                let hovered_index = this.hovered_index;
                                let editing_note_id = this.editing_note_id;
                                let recording_hotkey_id = this.recording_hotkey_id;
                                let recording_hotkey_format =
                                    this.recording_hotkey_format.clone();
                                let inline_locked_item_id = this.inline_locked_item_id();
                                let note_input = this.note_input.clone();
                                let (
                                    show_source_app,
                                    show_original_on_hover,
                                    show_page_title,
                                    search_terms,
                                    pending_transfer_uploads,
                                ) = {
                                    let state = this.state.read(cx);
                                    (
                                        state.settings.show_source_app,
                                        state.settings.show_original_on_hover,
                                        state.settings.auto_fetch_url_title,
                                        state.filters.keyword_terms(),
                                        state.pending_transfer_uploads.clone(),
                                    )
                                };
                                range
                                    .filter_map(|i| {
                                        let item = this.items.get(i)?;
                                        let item_id = item.id;
                                        let inline_locked = inline_locked_item_id == Some(item_id);
                                        let selected =
                                            inline_locked || this.selected_ids.contains(&item_id);
                                        let is_hovered = inline_locked
                                            || (inline_locked_item_id.is_none()
                                                && hovered_index == Some(i));
                                        let list_view = list_entity.clone();
                                        let focus_handle = this.focus_handle.clone();
                                        let item_clone = item.clone();
                                        let source_is_uploading = item_clone.content_type
                                            == ContentType::File
                                            && FileData::from_json(&item_clone.file_data)
                                                .files
                                                .iter()
                                                .any(|file| {
                                                    pending_transfer_uploads.contains(&file.path)
                                                });
                                        let hotkey_format = if recording_hotkey_id == item_id {
                                            recording_hotkey_format.clone()
                                        } else {
                                            item_clone.custom_hotkey_format.clone()
                                        };

                                        let click_handler = Rc::new(
                                            move |idx: usize,
                                                  modifiers: Modifiers,
                                                  window: &mut Window,
                                                  cx: &mut App| {
                                                focus_handle.focus(window);
                                                list_view.update(cx, move |this, cx| {
                                                    this.handle_card_click(idx, modifiers, window, cx);
                                                });
                                            },
                                        );

                                        let list_for_right = list_entity.clone();
                                        let list_for_hover = list_entity.clone();
                                        let list_for_toolbar = list_entity.clone();
                                        let list_for_note_commit = list_entity.clone();

                                        // Compute 1-based selection order for badge display
                                        let selection_order = this
                                            .selected_ids
                                            .iter()
                                            .position(|&id| id == item_id)
                                            .map(|p| p + 1)
                                            .unwrap_or(0);

                                        Some(
                                            div()
                                                .w_full()
                                                .h_full()
                                                .overflow_hidden()
                                                .py(px(4.))
                                                .on_scroll_wheel({
                                                    let list_for_scroll_lock =
                                                        list_entity.clone();
                                                    move |_ev, _window, cx| {
                                                        list_for_scroll_lock.update(
                                                            cx,
                                                            |this, cx| {
                                                                if this
                                                                    .inline_locked_item_id()
                                                                    .is_some()
                                                                {
                                                                    cx.stop_propagation();
                                                                }
                                                            },
                                                        );
                                                    }
                                                })
                                                .on_mouse_move({
                                                    move |ev, _window, cx| {
                                                        cx.stop_propagation();
                                                        let modifiers = ev.modifiers;
                                                        list_for_hover.update(cx, |this, cx| {
                                                            if this
                                                                .inline_locked_item_id()
                                                                .is_some()
                                                            {
                                                                return;
                                                            }
                                                            if this.hovered_index != Some(i) {
                                                                this.hovered_index = Some(i);
                                                                // --- Hover-to-select: when ≤1 item ---
                                                                // --- is selected and no multi-select ---
                                                                // --- modifier is held, hover drives  ---
                                                                // --- selection automatically.         ---
                                                                if this.selected_ids.len() <= 1
                                                                    && !additive_selection_modifier(modifiers)
                                                                    && !modifiers.shift
                                                                {
                                                                    if let Some(item) =
                                                                        this.items.get(i)
                                                                    {
                                                                        this.selected_ids.clear();
                                                                        this.selected_ids
                                                                            .push(item.id);
                                                                        this.selected_index =
                                                                            Some(i);
                                                                        this.anchor_index = Some(i);
                                                                        this.selected_count = 1;
                                                                        this.modified_double_click_candidate = None;
                                                                        let item_id = item.id;
                                                                        this.state.update(
                                                                            cx,
                                                                            |state, _cx| {
                                                                                state.select_single(
                                                                                    item_id,
                                                                                );
                                                                            },
                                                                        );
                                                                    }
                                                                }
                                                                cx.notify();
                                                            }
                                                            this.last_hover_pos = Some(ev.position);
                                                        });
                                                    }
                                                })
                                                .on_mouse_down(
                                                    MouseButton::Right,
                                                    move |ev: &MouseDownEvent, _window, cx| {
                                                        list_for_right.update(cx, |this, cx| {
                                                            this.modified_double_click_candidate = None;
                                                            #[cfg(target_os = "macos")]
                                                            if macos_control_modifier_pressed() {
                                                                this.toggle_index(i, cx);
                                                                return;
                                                            }

                                                            if let Some(item) = this.items.get(i) {
                                                                let already_selected = this
                                                                    .selected_ids
                                                                    .contains(&item.id);
                                                                let is_batch =
                                                                    this.selected_ids.len() > 1
                                                                        && already_selected;
                                                                if !already_selected {
                                                                    // --- Right-click on ---
                                                                    // unselected item →                                                                        // select it first
                                                                    this.selected_ids.clear();
                                                                    this.selected_ids.push(item.id);
                                                                    this.selected_index = Some(i);
                                                                    this.anchor_index = Some(i);
                                                                    this.selected_count = 1;
                                                                    let item_id = item.id;
                                                                    this.state.update(
                                                                        cx,
                                                                        move |state, _cx| {
                                                                            state.select_single(
                                                                                item_id,
                                                                            );
                                                                        },
                                                                    );
                                                                }
                                                                this.context_menu_visible = true;
                                                                this.context_menu_x =
                                                                    f32::from(ev.position.x);
                                                                this.context_menu_y =
                                                                    f32::from(ev.position.y);
                                                                this.context_menu_item =
                                                                    Some(item.clone());
                                                                this.context_menu_is_batch =
                                                                    is_batch;
                                                                cx.notify();
                                                            }
                                                        });
                                                    },
                                                )
                                                .child({
                                                    let card = ClipboardCard::new(
                                                        Rc::new(item_clone),
                                                        selected,
                                                        i,
                                                    )
                                                    .theme(theme.clone())
                                                    .hovered(is_hovered)
                                                    .show_source_app(show_source_app)
                                                    .show_original_on_hover(show_original_on_hover)
                                                    .show_page_title(show_page_title)
                                                    .source_is_uploading(source_is_uploading)
                                                    .image_cache(image_cache.clone())
                                                    .search_terms(search_terms.clone())
                                                    .selected_count(selected_count)
                                                    .can_merge_selection(can_merge_selection)
                                                    .selection_order(selection_order)
                                                    .editing(editing_note_id == item_id)
                                                    .recording_hotkey(recording_hotkey_id == item_id)
                                                    .hotkey_paste_format(hotkey_format)
                                                    .on_hotkey_format_change({
                                                        let list_for_format =
                                                            list_for_note_commit.clone();
                                                        move |format, _window, cx| {
                                                            list_for_format.update(
                                                                cx,
                                                                |this, cx| {
                                                                    this.update_recording_hotkey_format(
                                                                        format, cx,
                                                                    );
                                                                },
                                                            );
                                                        }
                                                    })
                                                    .on_commit_note({
                                                        let list_for_commit =
                                                            list_for_note_commit.clone();
                                                        move |window, cx| {
                                                            list_for_commit.update(
                                                                cx,
                                                                |this, cx| {
                                                                    this.commit_note_edit(
                                                                        window, cx,
                                                                    );
                                                                },
                                                            );
                                                        }
                                                    })
                                                    .on_toolbar_action(move |action, window, cx| {
                                                        list_for_toolbar.update(cx, |this, cx| {
                                                            this.handle_toolbar_action(
                                                                action, window, cx,
                                                            );
                                                        });
                                                    })
                                                    .on_double_click({
                                                        let list_for_dbl = list_entity.clone();
                                                        Rc::new(move |idx, modifiers, _window, cx| {
                                                            list_for_dbl.update(cx, |this, cx| {
                                                                this.handle_double_click(idx, modifiers, cx);
                                                            });
                                                        })
                                                    });

                                                    // --- Editing card: no click handler → Input receives clicks directly ---
                                                    // --- Normal card: full click handler + edit-commit check ---
                                                    if editing_note_id == item_id {
                                                        card.note_input(note_input.clone())
                                                    } else {
                                                        card.on_click(click_handler)
                                                    }
                                                    .on_commit_hotkey({
                                                        let lst = list_for_note_commit.clone();
                                                        move |_window, cx| {
                                                            lst.update(cx, |this, cx| {
                                                                this.cancel_hotkey_recording(cx);
                                                            });
                                                        }
                                                    })
                                                    .on_cancel_hotkey({
                                                        let lst = list_for_note_commit.clone();
                                                        move |_window, cx| {
                                                            lst.update(cx, |this, cx| {
                                                                this.cancel_hotkey_recording(cx);
                                                            });
                                                        }
                                                    })
                                                })
                                                .into_any_element(),
                                        )
                                    })
                                    .collect::<Vec<_>>()
                            },
                        )
                        .track_scroll(&self.scroll_handle)
                        .overflow_x_hidden(),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(4.))
                            .right(px(0.))
                            .bottom(px(10.))
                            .w(px(CLIPBOARD_SCROLLBAR_WIDTH))
                            .when(self.inline_locked_item_id().is_none(), |el| {
                                el.child(
                                    Scrollbar::vertical(&self.scroll_handle)
                                        .scrollbar_show(ScrollbarShow::Scrolling),
                                )
                            }),
                    )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{can_incrementally_sync_usage, item_id_at, item_index, reorder_usage_pairs};
    use crate::core::types::{ClipboardItem, ContentType};
    use gpui::Pixels;

    fn test_item(id: i64, updated_secs: i64, favorite: bool) -> ClipboardItem {
        let ts = chrono::Utc::now() - chrono::Duration::seconds(updated_secs);
        ClipboardItem {
            id,
            content_type: ContentType::PlainText,
            full_text: format!("item {id}"),
            content_hash: 0x100 + id as u64,
            created_at: ts,
            updated_at: ts,
            image_path: String::new(),
            image_width: 0,
            image_height: 0,
            rich_data: String::new(),
            file_data: String::new(),
            is_favorite: favorite,
            note: String::new(),
            source_app_name: String::new(),
            source_app_icon: String::new(),
            size: 0,
            tags: Vec::new(),
            meta_type: String::new(),
            custom_hotkey: String::new(),
            custom_hotkey_format: String::new(),
            existence_observed_at: String::new(),
        }
    }

    fn height(px: f32) -> gpui::Size<gpui::Pixels> {
        gpui::Size::new(Pixels::from(px), Pixels::from(px))
    }

    #[test]
    fn usage_reorder_moves_items_together_with_their_sizes() {
        let mut pairs = vec![
            (test_item(1, 100, false), height(100.0)),
            (test_item(2, 50, false), height(50.0)),
            (test_item(3, 10, false), height(10.0)),
        ];
        // Touch item 1: newest now, must move to the front.
        pairs[0].0.updated_at = chrono::Utc::now();
        reorder_usage_pairs(&mut pairs, false, false);

        let ids: Vec<i64> = pairs.iter().map(|(item, _)| item.id).collect();
        let sizes: Vec<Pixels> = pairs.iter().map(|(_, size)| size.width).collect();
        assert_eq!(ids, vec![1, 3, 2]);
        // Cached heights followed their items: id 1 keeps height 100.0.
        assert_eq!(
            sizes,
            vec![Pixels::from(100.0), Pixels::from(10.0), Pixels::from(50.0)]
        );
    }

    #[test]
    fn usage_reorder_keeps_positions_for_created_at_order() {
        let mut pairs = vec![
            (test_item(1, 100, false), height(100.0)),
            (test_item(2, 50, false), height(50.0)),
            (test_item(3, 10, false), height(10.0)),
        ];
        pairs[0].0.updated_at = chrono::Utc::now();
        reorder_usage_pairs(&mut pairs, true, false);

        let ids: Vec<i64> = pairs.iter().map(|(item, _)| item.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn usage_reorder_moves_only_within_favorites_first_groups() {
        let mut pairs = vec![
            (test_item(1, 100, true), height(100.0)),
            (test_item(2, 50, true), height(50.0)),
            (test_item(3, 10, false), height(10.0)),
            (test_item(4, 200, false), height(200.0)),
        ];
        // Touch the oldest favorite: moves inside the favorite group only.
        pairs[0].0.updated_at = chrono::Utc::now();
        reorder_usage_pairs(&mut pairs, false, true);

        let ids: Vec<i64> = pairs.iter().map(|(item, _)| item.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
        let sizes: Vec<Pixels> = pairs.iter().map(|(_, size)| size.width).collect();
        assert_eq!(
            sizes,
            vec![
                Pixels::from(100.0),
                Pixels::from(50.0),
                Pixels::from(10.0),
                Pixels::from(200.0)
            ]
        );
    }

    #[test]
    fn usage_reorder_restores_interaction_indices_by_item_id() {
        let mut pairs = vec![
            (test_item(1, 100, false), height(100.0)),
            (test_item(2, 50, false), height(50.0)),
            (test_item(3, 10, false), height(10.0)),
        ];
        let items_before: Vec<ClipboardItem> = pairs.iter().map(|(item, _)| item.clone()).collect();
        let selected_id = item_id_at(&items_before, Some(1));
        let anchor_id = item_id_at(&items_before, Some(0));
        let hovered_id = item_id_at(&items_before, Some(2));

        pairs[0].0.updated_at = chrono::Utc::now();
        reorder_usage_pairs(&mut pairs, false, false);
        let items_after: Vec<ClipboardItem> = pairs.into_iter().map(|(item, _)| item).collect();

        assert_eq!(item_index(&items_after, selected_id), Some(2));
        assert_eq!(item_index(&items_after, anchor_id), Some(0));
        assert_eq!(item_index(&items_after, hovered_id), Some(1));
    }

    #[test]
    fn usage_incremental_sync_requires_exact_ids_and_size_cache() {
        let local = vec![test_item(1, 20, false), test_item(2, 10, false)];
        assert!(can_incrementally_sync_usage(
            &local,
            2,
            &local,
            &[1],
            false,
            false
        ));

        let different_ids = vec![test_item(1, 20, false), test_item(3, 10, false)];
        assert!(!can_incrementally_sync_usage(
            &local,
            2,
            &different_ids,
            &[1],
            false,
            false
        ));
        assert!(!can_incrementally_sync_usage(
            &local,
            1,
            &local,
            &[1],
            false,
            false
        ));
    }

    #[test]
    fn usage_incremental_sync_rejects_forced_or_unmatched_updates() {
        let items = vec![test_item(1, 20, false), test_item(2, 10, false)];
        assert!(!can_incrementally_sync_usage(
            &items,
            2,
            &items,
            &[1],
            false,
            true
        ));
        assert!(!can_incrementally_sync_usage(
            &items,
            2,
            &items,
            &[99],
            false,
            false
        ));
    }
}
