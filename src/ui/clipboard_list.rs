//! Clipboard list — variable-height virtual scrolling list.
//!
//! Uses `gpui_component::v_virtual_list` to efficiently render thousands
//! of clipboard items with dynamic card heights (68–128px).

use std::rc::Rc;

use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::v_virtual_list;
use gpui_component::scroll::Scrollbar;
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
    selected_ids: Vec<i64>,
    state: Entity<AppState>,
}

impl ClipboardListView {
    pub fn new(items: Vec<ClipboardItem>, state: Entity<AppState>) -> Self {
        let item_sizes = Rc::new(Self::compute_sizes(&items, "medium"));
        Self {
            items,
            item_sizes,
            card_height_mode: "medium".into(),
            scroll_handle: VirtualListScrollHandle::new(),
            selected_ids: Vec::new(),
            state,
        }
    }

    pub fn set_items(&mut self, items: Vec<ClipboardItem>, cx: &mut Context<Self>) {
        self.item_sizes = Rc::new(Self::compute_sizes(&items, &self.card_height_mode));
        self.items = items;
        self.selected_ids.clear();
        cx.notify();
    }

    fn compute_sizes(items: &[ClipboardItem], mode: &str) -> Vec<Size<Pixels>> {
        let sizes: Vec<_> = items
            .iter()
            .map(|item| {
                let h = estimate_card_height(item, mode) + 10.0; // Slint ListView row padding: 5px top + bottom
                size(px(308.), px(h))
            })
            .collect();
        log::info!("ClipboardListView::compute_sizes: {} items", sizes.len());
        sizes
    }
}

impl Render for ClipboardListView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let item_sizes = self.item_sizes.clone();
        let items_count = self.item_sizes.len();
        let view = cx.entity();
        let list_entity = view.clone();

        let empty_state = items_count == 0;

        div()
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
                                        let state = this.state.clone();
                                        let list_view = list_entity.clone();
                                        let handler: Rc<dyn Fn(usize, &mut Window, &mut App)> =
                                            Rc::new(move |idx, _window, cx| {
                                                log::info!("Clicked item at index {idx}");
                                                let _ = state.update(cx, move |state, _cx| {
                                                    state.select_single(item_id);
                                                });
                                                let _ = list_view.update(cx, move |this, cx| {
                                                    this.selected_ids.clear();
                                                    this.selected_ids.push(item_id);
                                                    cx.notify();
                                                });
                                            });
                                        Some(
                                            div()
                                                .w_full()
                                                .h_full()
                                                .py(px(5.))
                                                .child(
                                                    ClipboardCard::new(item.clone(), selected, i)
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
    }
}
