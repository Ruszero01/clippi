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
    state: Entity<AppState>,
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
            state,
        }
    }

    pub fn set_items(&mut self, items: Vec<ClipboardItem>, cx: &mut Context<Self>) {
        self.item_sizes = Rc::new(Self::compute_sizes(&items, &self.card_height_mode));
        self.items = items;
        self.selected_ids.clear();
        self.selected_index = None;
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
        let _ = self.state.update(cx, move |state, _cx| {
            state.select_single(item_id);
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
            .px(px(8.))
            .pt(px(4.))
            .pb(px(8.))
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
                                range
                                    .filter_map(|i| {
                                        let item = this.items.get(i)?;
                                        let item_id = item.id;
                                        let selected = this.selected_ids.contains(&item_id);
                                        let list_view = list_entity.clone();
                                        let focus_handle = this.focus_handle.clone();
                                        let handler: Rc<dyn Fn(usize, &mut Window, &mut App)> =
                                            Rc::new(move |idx, window, cx| {
                                                focus_handle.focus(window);
                                                let _ = list_view.update(cx, move |this, cx| {
                                                    this.select_index_without_scroll(idx, cx);
                                                });
                                            });
                                        Some(
                                            div()
                                                .w_full()
                                                .h_full()
                                                .py(px(5.))
                                                .child(
                                                    ClipboardCard::new(Rc::new(item.clone()), selected, i)
                                                        .on_click(handler),
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
