//! --- Clipboard list — variable-height virtual scrolling list. ---
//!
//! --- Uses `gpui_component::v_virtual_list` to efficiently render thousands ---
//! --- of clipboard items with dynamic card heights (68— 28px). ---

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::InputState;
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use gpui_component::v_virtual_list;
use gpui_component::VirtualListScrollHandle;

use crate::core::i18n_keys::I18nKey;
use crate::core::types::ClipboardItem;
use crate::state::app::AppState;

use super::clipboard_card::{estimate_card_height, ClipboardCard};
use super::search_bar::SearchBar;
use super::tag_picker::TagState;
use super::theme::ClippiTheme;

const CLIPBOARD_ROW_VERTICAL_SPACE: f32 = 16.0;
const CLIPBOARD_BOTTOM_SCROLL_INSET: f32 = 36.0;
const CLIPBOARD_SCROLLBAR_WIDTH: f32 = 16.0;

/// Types of confirmation dialogs that can be shown.
/// [FUTURE] Add variants here for other confirmation scenarios
/// (e.g. RemoveBlacklist { app_name: String } for hotkey settings).
#[derive(Clone)]
pub(crate) enum ConfirmDialogState {
    DeleteSingle { id: i64 },
    DeleteBatch { count: usize },
}

pub enum ClipboardListEvent {
    OpenEdit(i64),
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
    /// Reference to the search bar for programmatic focus (Ctrl+F).
    /// Set after construction to resolve circular dependency.
    pub(crate) search_bar: Option<Entity<SearchBar>>,
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
    /// Active confirmation dialog (None = hidden).
    confirm_dialog: Option<ConfirmDialogState>,
    /// Persisted last-selected item ID — survives across set_items calls
    /// (including the empty-item clear during window hide).
    last_selected_id: i64,
    theme: ClippiTheme,
    last_lang_version: u64,
}

impl ClipboardListView {
    pub fn new(
        items: Vec<ClipboardItem>,
        state: Entity<AppState>,
        theme: ClippiTheme,
        window: &mut Window,
        cx: &mut App,
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
            confirm_dialog: None,
            last_selected_id: -1,
            theme,
            last_lang_version: crate::core::i18n::lang_version(),
        }
    }

    pub fn set_items(&mut self, items: Vec<ClipboardItem>, cx: &mut Context<Self>) {
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

    /// Reload local items from AppState without resetting UI state.
    /// Use after mutations (toggle_favorite, tag ops, delete) to keep the list
    /// in sync. Items retain their current order; re-sort on next window open.
    pub(crate) fn sync_items_from_state(&mut self, cx: &mut Context<Self>) {
        let app_items = self.state.read(cx).items.clone();
        self.item_sizes = Rc::new(Self::compute_sizes(&app_items, &self.card_height_mode));
        self.items = app_items;
        cx.notify();
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
        self.state.update(cx, move |state, _cx| {
            state.select_single(item_id);
        });
        cx.notify();
    }

    /// Toggle selection of an item (Ctrl+click).
    fn toggle_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(index) else {
            return;
        };
        let item_id = item.id;
        if let Some(pos) = self.selected_ids.iter().position(|&x| x == item_id) {
            self.selected_ids.remove(pos);
            if self.selected_ids.is_empty() {
                self.anchor_index = None;
                self.selected_index = None;
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

    /// Paste selected item(s). Called from list key handler and search bar.
    pub(crate) fn action_paste(&mut self, plain: bool, cx: &mut Context<Self>) {
        if self.selected_count > 1 {
            let ids = self.selected_ids.clone();
            self.state.update(cx, |s, _cx| s.batch_paste(&ids, plain));
        } else if let Some(idx) = self.selected_index {
            if let Some(item) = self.items.get(idx) {
                let id = item.id;
                if plain {
                    self.state.update(cx, |s, _cx| s.paste_item_plain(id));
                } else {
                    self.state.update(cx, |s, _cx| s.paste_item(id, plain));
                }
            }
        }
    }

    /// Toggle favorite on selected item(s).
    pub(crate) fn action_toggle_favorite(&mut self, cx: &mut Context<Self>) {
        if self.selected_count > 1 {
            self.state.update(cx, |s, _cx| s.batch_toggle_favorite());
        } else if let Some(idx) = self.selected_index {
            if let Some(item) = self.items.get(idx) {
                let id = item.id;
                self.state.update(cx, |s, _cx| s.toggle_favorite(id));
            }
        }
        self.sync_items_from_state(cx);
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

    /// Show the delete confirmation dialog for selected item(s).
    pub(crate) fn action_delete(&mut self, cx: &mut Context<Self>) {
        if self.selected_count > 1 {
            let count = self.selected_ids.len();
            self.confirm_dialog = Some(ConfirmDialogState::DeleteBatch { count });
        } else if let Some(idx) = self.selected_index {
            if let Some(item) = self.items.get(idx) {
                self.confirm_dialog = Some(ConfirmDialogState::DeleteSingle { id: item.id });
            }
        }
        cx.notify();
    }

    /// Clear the current selection. Called from search bar Escape handler.
    pub(crate) fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selected_ids.clear();
        self.selected_count = 0;
        self.selected_index = None;
        self.anchor_index = None;
        cx.notify();
    }

    // --- Context menu state accessors ---

    pub fn context_menu_visible(&self) -> bool {
        self.context_menu_visible
    }

    pub fn context_menu_is_batch(&self) -> bool {
        self.context_menu_is_batch
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

    fn show_tag_picker(&mut self, is_batch: bool, x: f32, y: f32, cx: &mut Context<Self>) {
        self.tag_picker_visible = true;
        self.tag_picker_x = x;
        self.tag_picker_y = y;
        self.tag_picker_is_batch = is_batch;
        self.tag_picker_item_id = self.context_menu_item.as_ref().map_or(-1, |item| item.id);
        cx.notify();
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
        // --- Keep the affected item selected and visible. ---
        // --- (updated_at is bumped but position is preserved — see AppState) ---
        self.scroll_to_item_if_visible(target_id, cx);
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
        self.scroll_to_item_if_visible(target_id, cx);
    }

    /// After a tag/favorite/note operation bumps `updated_at`, keep the affected
    /// item selected and in view. The item retains its current list position;
    /// re-sort happens on next window open.
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
        let text = SharedString::from(initial_text.to_string());
        self.note_input.update(cx, move |input, cx| {
            input.set_value(text, window, cx);
            // --- Auto-focus so user can type immediately ---
            input.focus_handle(cx).focus(window);
        });
        cx.notify();
    }

    /// Commit the current note edit to DB and exit edit mode.
    fn commit_note_edit(&mut self, cx: &mut Context<Self>) {
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
                }
            }
            "paste" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_item(item_id, plain));
                }
            }
            "paste_plain" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_item_plain(item_id));
                }
            }
            "paste_as_rgb" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_as_rgb(item_id));
                }
            }
            "paste_as_hex" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_as_hex(item_id));
                }
            }
            "batch_paste" => {
                let ids = self.selected_ids.clone();
                self.state.update(cx, |s, _cx| s.batch_paste(&ids, plain));
            }
            "edit_note" => {
                if let Some(ref item) = self.context_menu_item {
                    self.start_note_edit(item.id, &item.note.clone(), _window, cx);
                }
            }
            "toggle_favorite" => {
                if let Some(ref item) = self.context_menu_item {
                    let id = item.id;
                    self.state.update(cx, |s, _cx| s.toggle_favorite(id));
                    self.sync_items_from_state(cx);
                }
            }
            "delete" => {
                if let Some(ref item) = self.context_menu_item {
                    let id = item.id;
                    self.hide_context_menu(cx);
                    self.confirm_dialog = Some(ConfirmDialogState::DeleteSingle { id });
                    return; // Don't call hide_context_menu again at end
                }
            }
            "batch_favorite" => {
                self.state.update(cx, |s, _cx| s.batch_toggle_favorite());
                self.sync_items_from_state(cx);
            }
            "batch_delete" => {
                let count = self.selected_ids.len();
                self.confirm_dialog = Some(ConfirmDialogState::DeleteBatch { count });
                cx.notify();
            }
            "open_image" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state
                        .update(cx, |s, _cx| s.open_original_image(item_id));
                }
            }
            "paste_ocr" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_ocr(item_id));
                    self.sync_items_from_state(cx);
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
                self.show_tag_picker(
                    self.context_menu_is_batch,
                    self.context_menu_x,
                    self.context_menu_y,
                    cx,
                );
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
        let plain = self.state.read(cx).settings.copy_as_plain_text;
        match action {
            "copy" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        let item_id = item.id;
                        self.state.update(cx, |s, _cx| s.copy_item(item_id, plain));
                    }
                }
            }
            "paste_plain" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        let item_id = item.id;
                        self.state.update(cx, |s, _cx| s.paste_item_plain(item_id));
                    }
                }
            }
            // --- Batch toolbar actions ---
            "batch_paste" => {
                let ids = self.selected_ids.clone();
                self.state.update(cx, |s, _cx| s.batch_paste(&ids, plain));
            }
            "batch_tag" => {
                self.tag_picker_visible = true;
                let (px, py) = self
                    .last_hover_pos
                    .map(|p| (f32::from(p.x), f32::from(p.y)))
                    .unwrap_or((400.0, 80.0));
                self.tag_picker_x = px;
                self.tag_picker_y = py;
                self.tag_picker_is_batch = true;
                self.tag_picker_item_id = -1;
                cx.notify();
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
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        self.tag_picker_visible = true;
                        let (px, py) = self
                            .last_hover_pos
                            .map(|p| (f32::from(p.x), f32::from(p.y)))
                            .unwrap_or((400.0, 80.0));
                        self.tag_picker_x = px;
                        self.tag_picker_y = py;
                        self.tag_picker_is_batch = false;
                        self.tag_picker_item_id = item.id;
                        cx.notify();
                    }
                }
            }
            "toggle_favorite" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        self.state.update(cx, |s, _cx| s.toggle_favorite(item.id));
                        self.sync_items_from_state(cx);
                    }
                }
            }
            "delete" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        self.confirm_dialog =
                            Some(ConfirmDialogState::DeleteSingle { id: item.id });
                        cx.notify();
                    }
                }
            }
            "batch_favorite" => {
                self.state.update(cx, |s, _cx| s.batch_toggle_favorite());
                self.sync_items_from_state(cx);
            }
            "batch_delete" => {
                let count = self.selected_ids.len();
                self.confirm_dialog = Some(ConfirmDialogState::DeleteBatch { count });
                cx.notify();
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

        div()
            .flex_1()
            .overflow_hidden()
            .track_focus(&focus_handle)
            .on_key_down(
                window.listener_for(&view, |this, event: &KeyDownEvent, window, cx| {
                    let key = event.keystroke.key.as_str();
                    let ctrl = event.keystroke.modifiers.control;
                    let shift = event.keystroke.modifiers.shift;

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
                            _ => {}
                        }
                        return;
                    }

                    // --- Shift+Enter — paste as plain text ---
                    if shift && key == "enter" {
                        if this.has_any_panel_or_editing() {
                            this.dismiss_all_panels(cx);
                            cx.stop_propagation();
                        } else {
                            this.action_paste(true, cx);
                            cx.stop_propagation();
                        }
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
                            // --- Only paste when no floating panel or inline editing is active ---
                            if this.has_any_panel_or_editing() {
                                this.dismiss_all_panels(cx);
                                cx.stop_propagation();
                            } else {
                                let plain = this.state.read(cx).settings.copy_as_plain_text;
                                this.action_paste(plain, cx);
                                cx.stop_propagation();
                            }
                        }
                        "f2" if !this.has_any_panel_or_editing() => {
                            // F2 — add/edit note for selected item
                            this.action_edit_note(window, cx);
                            cx.stop_propagation();
                        }
                        "delete" if !this.has_any_panel_or_editing() => {
                            // Delete — remove selected item(s)
                            this.action_delete(cx);
                            cx.stop_propagation();
                        }
                        "escape" => {
                            // Escape — dismiss panels or clear selection
                            if this.has_any_panel_or_editing() {
                                this.dismiss_all_panels(cx);
                                this.editing_note_id = -1;
                                this.confirm_dialog = None;
                                cx.notify();
                            } else if !this.selected_ids.is_empty() {
                                this.clear_selection(cx);
                                // Sync to AppState
                                let empty: Vec<i64> = Vec::new();
                                this.state.update(cx, |s, _cx| {
                                    s.range_select(&empty);
                                });
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
            .on_mouse_move({
                let list_for_clear = list_entity.clone();
                move |_ev, _window, cx| {
                    list_for_clear.update(cx, |this, cx| {
                        if this.hovered_index.is_some() {
                            this.hovered_index = None;
                            cx.notify();
                        }
                    });
                }
            })
            .pt(px(4.))
            .pb(px(14.))
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
                            move |this, range, _window, _cx| {
                                let selected_count = this.selected_count;
                                let hovered_index = this.hovered_index;
                                let editing_note_id = this.editing_note_id;
                                let note_input = this.note_input.clone();
                                let settings = &this.state.read(_cx).settings;
                                let show_source_app = settings.show_source_app;
                                let show_original_on_hover = settings.show_original_on_hover;
                                range
                                    .filter_map(|i| {
                                        let item = this.items.get(i)?;
                                        let item_id = item.id;
                                        let selected = this.selected_ids.contains(&item_id);
                                        let is_hovered = hovered_index == Some(i);
                                        let list_view = list_entity.clone();
                                        let focus_handle = this.focus_handle.clone();
                                        let item_clone = item.clone();

                                        let click_handler = Rc::new(
                                            move |idx: usize,
                                                  modifiers: Modifiers,
                                                  window: &mut Window,
                                                  cx: &mut App| {
                                            focus_handle.focus(window);
                                            list_view.update(cx, move |this, cx| {
                                                // --- Clicking another card while editing → commit first ---
                                                if this.editing_note_id > 0 {
                                                    this.commit_note_edit(cx);
                                                }
                                                if modifiers.control {
                                                    this.toggle_index(idx, cx);
                                                } else if modifiers.shift {
                                                    this.range_select_to_index(idx, cx);
                                                } else {
                                                    this.select_index_without_scroll(idx, cx);
                                                }
                                            });
                                        });

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
                                                .py(px(5.))
                                                .on_mouse_move({
                                                    move |ev, _window, cx| {
                                                        cx.stop_propagation();
                                                        list_for_hover.update(cx, |this, cx| {
                                                            if this.hovered_index != Some(i) {
                                                                this.hovered_index = Some(i);
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
                                                    .selected_count(selected_count)
                                                    .selection_order(selection_order)
                                                    .editing(editing_note_id == item_id)
                                                    .on_commit_note({
                                                        let list_for_commit =
                                                            list_for_note_commit.clone();
                                                        move |_window, cx| {
                                                            list_for_commit.update(
                                                                cx,
                                                                |this, cx| {
                                                                    this.commit_note_edit(cx);
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
                                                        Rc::new(move |idx, _window, cx| {
                                                            list_for_dbl.update(cx, |this, cx| {
                                                                // Double-click always pastes with full formatting —
                                                                // user chose this specific item deliberately.
                                                                if this.selected_count > 1 {
                                                                    let ids =
                                                                        this.selected_ids.clone();
                                                                    this.state.update(
                                                                        cx,
                                                                        |s, _cx| {
                                                                            s.batch_paste(
                                                                                &ids, false,
                                                                            );
                                                                        },
                                                                    );
                                                                } else if let Some(item) =
                                                                    this.items.get(idx)
                                                                {
                                                                    let item_id = item.id;
                                                                    this.state.update(
                                                                        cx,
                                                                        |s, _cx| {
                                                                            s.paste_item(
                                                                                item_id, false,
                                                                            );
                                                                        },
                                                                    );
                                                                }
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
                                                })
                                                .into_any_element(),
                                        )
                                    })
                                    .collect::<Vec<_>>()
                            },
                        )
                        .track_scroll(&self.scroll_handle),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(4.))
                            .right(px(0.))
                            .bottom(px(10.))
                            .w(px(CLIPBOARD_SCROLLBAR_WIDTH))
                            .child(
                                Scrollbar::vertical(&self.scroll_handle)
                                    .scrollbar_show(ScrollbarShow::Scrolling),
                            ),
                    )
            })
            .into_any_element()
    }
}
