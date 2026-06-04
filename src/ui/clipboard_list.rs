//! Clipboard list — variable-height virtual scrolling list.
//!
//! Uses `gpui_component::v_virtual_list` to efficiently render thousands
//! of clipboard items with dynamic card heights (68–128px).

use std::rc::Rc;

use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::scroll::Scrollbar;
use gpui_component::v_virtual_list;
use gpui_component::VirtualListScrollHandle;

use crate::core::types::ClipboardItem;
use crate::state::app::AppState;

use super::clipboard_card::{ClipboardCard, estimate_card_height};

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
    // ── Hover tracking ──
    hovered_index: Option<usize>,
    // ── Context menu state ──
    context_menu_visible: bool,
    context_menu_x: f32,
    context_menu_y: f32,
    context_menu_item: Option<ClipboardItem>,
    context_menu_is_batch: bool,
    // ── Cached selected count ──
    pub(crate) selected_count: usize,
}

impl ClipboardListView {
    pub fn new(items: Vec<ClipboardItem>, state: Entity<AppState>, cx: &mut App) -> Self {
        let item_sizes = Rc::new(Self::compute_sizes(&items, "auto"));
        Self {
            items,
            item_sizes,
            card_height_mode: "auto".into(),
            scroll_handle: VirtualListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            selected_ids: Vec::new(),
            selected_index: None,
            anchor_index: None,
            state,
            hovered_index: None,
            context_menu_visible: false,
            context_menu_x: 0.0,
            context_menu_y: 0.0,
            context_menu_item: None,
            context_menu_is_batch: false,
            selected_count: 0,
        }
    }

    pub fn set_items(&mut self, items: Vec<ClipboardItem>, cx: &mut Context<Self>) {
        self.item_sizes = Rc::new(Self::compute_sizes(&items, &self.card_height_mode));
        self.items = items;
        self.selected_ids.clear();
        self.selected_index = None;
        self.anchor_index = None;
        self.selected_count = 0;
        self.hovered_index = None;
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
        let _ = self.state.update(cx, move |state, _cx| {
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
        let _ = self.state.update(cx, move |state, _cx| {
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
        let _ = self.state.update(cx, move |state, _cx| {
            state.range_select(&selected);
        });
        cx.notify();
    }

    /// Range select from anchor to given index (Shift+click).
    fn range_select_to_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let anchor = match self.anchor_index {
            Some(a) => a,
            None => {
                // No anchor — fall back to single select
                self.select_index_without_scroll(index, cx);
                return;
            }
        };
        self.selected_ids.clear();
        if anchor <= index {
            // Selecting downward: anchor (top) gets #1, count goes down
            for i in anchor..=index {
                if let Some(item) = self.items.get(i) {
                    self.selected_ids.push(item.id);
                }
            }
        } else {
            // Selecting upward: anchor (bottom) gets #1, count goes up
            for i in (index..=anchor).rev() {
                if let Some(item) = self.items.get(i) {
                    self.selected_ids.push(item.id);
                }
            }
        }
        self.selected_index = Some(index);
        self.selected_count = self.selected_ids.len();
        let selected = self.selected_ids.clone();
        let _ = self.state.update(cx, move |state, _cx| {
            state.range_select(&selected);
        });
        cx.notify();
    }

    fn select_next(&mut self, scroll_strategy: ScrollStrategy, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        let next_index = self
            .selected_index
            .map(|index| (index + 1).min(self.items.len().saturating_sub(1)))
            .unwrap_or(0);
        self.select_index(next_index, scroll_strategy, cx);
    }

    fn select_previous(&mut self, scroll_strategy: ScrollStrategy, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        let previous_index = self
            .selected_index
            .map(|index| index.saturating_sub(1))
            .unwrap_or(0);
        self.select_index(previous_index, scroll_strategy, cx);
    }

    // ── Context menu state accessors ──

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

    pub(crate) fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu_visible = false;
        self.context_menu_item = None;
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
                self.state
                    .update(cx, |s, _cx| s.batch_paste(&ids, plain));
            }
            // Other actions deferred to follow-up
            "edit" | "edit_note" | "toggle_favorite" | "delete"
            | "open_image" | "paste_ocr" | "qr_detect" | "show_tag_picker"
            | "batch_favorite" | "batch_delete" => {}
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
            // Batch toolbar actions
            "batch_paste" => {
                let ids = self.selected_ids.clone();
                self.state
                    .update(cx, |s, _cx| s.batch_paste(&ids, plain));
            }
            // Other hover toolbar actions deferred to follow-up
            "open_image" | "qr_action" | "open_location" | "edit"
            | "edit_note" | "toggle_favorite" | "delete"
            | "batch_favorite" | "batch_delete" => {}
            _ => {}
        }
    }

    fn compute_sizes(items: &[ClipboardItem], mode: &str) -> Vec<Size<Pixels>> {
        let sizes: Vec<_> = items
            .iter()
            .map(|item| {
                let h = estimate_card_height(item, mode) + 10.0; // Slint ListView row padding: 5px top + bottom
                size(px(308.), px(h))
            })
            .collect();
        sizes
    }
}

impl Render for ClipboardListView {
    #[allow(refining_impl_trait_reachable)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let item_sizes = self.item_sizes.clone();
        let items_count = self.item_sizes.len();
        let view = cx.entity();
        let focus_handle = self.focus_handle.clone();

        let empty_state = items_count == 0;

        // Empty state — render a lightweight static placeholder, completely
        // avoiding the virtual list subtree so GPUI releases its element cache.
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
                .bg(rgb(0x191a1b))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(0x919496))
                        .child("No items yet"),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(0x5f6264))
                        .child("Copied items will appear here"),
                )
                .into_any_element();
        }

        // Normal state — virtual scrolling list
        let list_entity = view.clone();

        div()
            .track_focus(&focus_handle)
            .on_key_down(window.listener_for(&view, |this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "up" => {
                        this.select_previous(ScrollStrategy::Top, cx);
                        cx.stop_propagation();
                    }
                    "down" => {
                        this.select_next(ScrollStrategy::Bottom, cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .flex_1()
            .h_full()
            .w_full()
            .overflow_hidden()
            .rounded_b(px(12.))
            .bg(rgb(0x191a1b))
            .on_mouse_move({
                let list_for_clear = list_entity.clone();
                move |_ev, _window, cx| {
                    let _ = list_for_clear.update(cx, |this, cx| {
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
                                .text_color(rgb(0x919496))
                                .child("No items yet"),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(0x5f6264))
                                .child("Copied items will appear here"),
                        ),
                )
            })
            .when(!empty_state, |el| {
                el.relative()
                    .h_full()
                    .overflow_hidden()
                    .child(
                        v_virtual_list(
                            view.clone(),
                            "clippi-clipboard-list",
                            item_sizes,
                            move |this, range, _window, _cx| {
                                let selected_count = this.selected_count;
                                let hovered_index = this.hovered_index;
                                range
                                    .filter_map(|i| {
                                        let item = this.items.get(i)?;
                                        let item_id = item.id;
                                        let selected = this.selected_ids.contains(&item_id);
                                        let is_hovered = hovered_index == Some(i);
                                        let list_view = list_entity.clone();
                                        let focus_handle = this.focus_handle.clone();
                                        let item_clone = item.clone();

                                        let click_handler: Rc<
                                            dyn Fn(usize, Modifiers, &mut Window, &mut App),
                                        > = Rc::new(
                                            move |idx, modifiers, window, cx| {
                                                focus_handle.focus(window);
                                                let _ = list_view.update(
                                                    cx,
                                                    move |this, cx| {
                                                        if modifiers.control {
                                                            this.toggle_index(idx, cx);
                                                        } else if modifiers.shift {
                                                            this.range_select_to_index(idx, cx);
                                                        } else {
                                                            this.select_index_without_scroll(
                                                                idx, cx,
                                                            );
                                                        }
                                                    },
                                                );
                                            },
                                        );

                                        let list_for_right = list_entity.clone();
                                        let list_for_hover = list_entity.clone();
                                        let list_for_toolbar = list_entity.clone();

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
                                                    move |_ev, _window, cx| {
                                                        cx.stop_propagation();
                                                        let _ = list_for_hover.update(
                                                            cx,
                                                            |this, cx| {
                                                                if this.hovered_index != Some(i) {
                                                                    this.hovered_index = Some(i);
                                                                    cx.notify();
                                                                }
                                                            },
                                                        );
                                                    }
                                                })
                                                .on_mouse_down(
                                                    MouseButton::Right,
                                                    move |ev: &MouseDownEvent,
                                                          _window,
                                                          cx| {
                                                        let _ = list_for_right.update(
                                                            cx,
                                                            |this, cx| {
                                                                if let Some(item) =
                                                                    this.items.get(i)
                                                                {
                                                                    let already_selected =
                                                                        this.selected_ids
                                                                            .contains(&item.id);
                                                                    let is_batch = this
                                                                        .selected_ids
                                                                        .len()
                                                                        > 1
                                                                        && already_selected;
                                                                    if !already_selected {
                                                                        // Right-click on
                                                                        // unselected item →
                                                                        // select it first
                                                                        this.selected_ids
                                                                            .clear();
                                                                        this.selected_ids
                                                                            .push(item.id);
                                                                        this.selected_index =
                                                                            Some(i);
                                                                        this.anchor_index = Some(i);
                                                                        this.selected_count = 1;
                                                                        let item_id = item.id;
                                                                        let _ = this.state.update(
                                                                            cx,
                                                                            move |state, _cx| {
                                                                                state.select_single(
                                                                                    item_id,
                                                                                );
                                                                            },
                                                                        );
                                                                    }
                                                                    this.context_menu_visible =
                                                                        true;
                                                                    this.context_menu_x =
                                                                        f32::from(
                                                                            ev.position.x,
                                                                        );
                                                                    this.context_menu_y =
                                                                        f32::from(
                                                                            ev.position.y,
                                                                        );
                                                                    this.context_menu_item =
                                                                        Some(item.clone());
                                                                    this.context_menu_is_batch =
                                                                        is_batch;
                                                                    cx.notify();
                                                                }
                                                            },
                                                        );
                                                    },
                                                )
                                                .child(
                                                    ClipboardCard::new(
                                                        Rc::new(item_clone),
                                                        selected,
                                                        i,
                                                    )
                                                    .hovered(is_hovered)
                                                    .selected_count(selected_count)
                                                    .selection_order(selection_order)
                                                    .on_click(click_handler)
                                                    .on_toolbar_action(
                                                        move |action, window, cx| {
                                                            let _ = list_for_toolbar.update(
                                                                cx,
                                                                |this, cx| {
                                                                    this.handle_toolbar_action(
                                                                        action, window, cx,
                                                                    );
                                                                },
                                                            );
                                                        },
                                                    )
                                                    .on_double_click({
                                                    let list_for_dbl = list_entity.clone();
                                                    Rc::new(
                                                        move |idx, _window, cx| {
                                                            let _ = list_for_dbl.update(
                                                                cx,
                                                                |this, cx| {
                                                                    let plain = this
                                                                        .state
                                                                        .read(cx)
                                                                        .settings
                                                                        .copy_as_plain_text;
                                                                    if this.selected_count > 1 {
                                                                        let ids = this
                                                                            .selected_ids
                                                                            .clone();
                                                                        this.state.update(
                                                                            cx,
                                                                            |s, _cx| {
                                                                                s.batch_paste(
                                                                                    &ids, plain,
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
                                                                                    item_id,
                                                                                    plain,
                                                                                );
                                                                            },
                                                                        );
                                                                    }
                                                                },
                                                            );
                                                        },
                                                    )
                                                    })
                                                )
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
                            .top(px(0.))
                            .right(px(0.))
                            .bottom(px(0.))
                            .w(px(6.))
                            .child(
                                Scrollbar::vertical(&self.scroll_handle),
                            ),
                    )
            })
            .into_any_element()
    }
}
