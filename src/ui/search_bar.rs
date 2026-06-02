//! Search bar - keyword input + content type filter buttons.
//!
//! Matches the original Slint ClipboardList.slint design:
//! - 28px search box, 10px border-radius, surface bg, divider border
//! - Filter buttons: 22px height, 5px border-radius, icon+label layout
//! - Active state: accent text + 22% accent bg overlay
//! - Tag filter button at right end

use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};

use crate::state::app::AppState;

use super::clipboard_list::ClipboardListView;
use super::theme::ClippiTheme;

struct FilterDef {
    label: &'static str,
    key: &'static str,
    icon: &'static str,
}

const FILTER_TYPES: &[FilterDef] = &[
    FilterDef { label: "Text", key: "plain_text", icon: "\u{e60e}" },
    FilterDef { label: "RTF", key: "rich_text", icon: "\u{e6ae}" },
    FilterDef { label: "Files", key: "file", icon: "\u{e646}" },
    FilterDef { label: "Links", key: "link", icon: "\u{e6d7}" },
    FilterDef { label: "Color", key: "color", icon: "\u{e610}" },
];

pub struct SearchBar {
    input: Entity<InputState>,
    state: Entity<AppState>,
    list_view: Entity<ClipboardListView>,
    tag_panel_open: bool,
    theme: ClippiTheme,
    _subscriptions: Vec<Subscription>,
}

impl SearchBar {
    pub fn new(
        state: Entity<AppState>,
        list_view: Entity<ClipboardListView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search clipboard..."));
        let state_for_input = state.clone();
        let list_for_input = list_view.clone();
        let input_for_read = input.clone();

        let _subscriptions = vec![cx.subscribe(&input, move |_this, _, ev: &InputEvent, cx| {
            if !matches!(ev, InputEvent::Change) {
                return;
            }

            let keyword = input_for_read.read(cx).value().to_string();
            let items = state_for_input.update(cx, |state, _cx| {
                state.set_keyword(&keyword);
                state.items.clone()
            });
            list_for_input.update(cx, |list, cx| list.set_items(items, cx));
            cx.notify();
        })];

        Self {
            input,
            state,
            list_view,
            tag_panel_open: false,
            theme: ClippiTheme::dark(),
            _subscriptions,
        }
    }

    pub fn tag_panel_open(&self) -> bool {
        self.tag_panel_open
    }

    pub fn close_tag_panel(&mut self, cx: &mut Context<Self>) {
        self.tag_panel_open = false;
        cx.notify();
    }

    fn apply_type_filter(
        state: &Entity<AppState>,
        list_view: &Entity<ClipboardListView>,
        type_name: &'static str,
        cx: &mut App,
    ) {
        let items = state.update(cx, |state, _cx| {
            state.toggle_type_filter(type_name);
            state.items.clone()
        });
        list_view.update(cx, |list, cx| list.set_items(items, cx));
    }
}

impl Render for SearchBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        let accent = theme.accent;
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;
        let surface = theme.surface;
        let divider = theme.divider;
        let this = cx.entity().clone();
        let state_snapshot = self.state.read(cx);
        let has_tag_filter = !state_snapshot.filters.tag_ids.is_empty();

        div()
            .flex()
            .flex_col()
            .w_full()
            .flex_shrink_0()
            .pt(px(4.))
            .px(px(8.))
            .pb(px(8.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .h(px(28.))
                    .bg(surface)
                    .rounded(px(10.))
                    .border(px(1.))
                    .border_color(divider)
                    .mb(px(6.))
                    .overflow_hidden()
                    .child(
                        Input::new(&self.input)
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false)
                            .w_full()
                            .h_full()
                            .px(px(0.))
                            .text_size(px(12.))
                            .prefix(
                                div()
                                    .pl(px(8.))
                                    .pr(px(3.))
                                    .text_size(px(14.))
                                    .font_family("iconfont")
                                    .text_color(text_3)
                                    .child("\u{e688}"),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(22.))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(6.))
                            .children(FILTER_TYPES.iter().map(|f| {
                                let is_active = if f.key == "file" {
                                    state_snapshot.filters.is_type_active("file")
                                        || state_snapshot.filters.is_type_active("image")
                                } else {
                                    state_snapshot.filters.is_type_active(f.key)
                                };
                                let filter_bg = if is_active {
                                    theme.accent_overlay()
                                } else {
                                    rgba(0x00000000)
                                };
                                let filter_text = if is_active { accent } else { text_2 };
                                let filter_weight = if is_active {
                                    FontWeight::BOLD
                                } else {
                                    FontWeight::default()
                                };
                                let state = self.state.clone();
                                let list_view = self.list_view.clone();
                                let this = this.clone();
                                let key = f.key;

                                div()
                                    .h(px(22.))
                                    .rounded(px(5.))
                                    .bg(filter_bg)
                                    .px(px(6.))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(3.))
                                    .cursor(CursorStyle::PointingHand)
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                        Self::apply_type_filter(&state, &list_view, key, cx);
                                        let _ = this.update(cx, |_bar, cx| cx.notify());
                                    })
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_family("iconfont")
                                            .text_color(filter_text)
                                            .child(f.icon.to_string()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_weight(filter_weight)
                                            .text_color(filter_text)
                                            .child(f.label.to_string()),
                                    )
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(2.))
                            .child(
                                div()
                                    .w(px(1.))
                                    .h(px(14.))
                                    .bg(if theme.bg == rgb(0x191a1b) {
                                        rgba(0xffffff18)
                                    } else {
                                        rgba(0x00000014)
                                    }),
                            )
                            .child(
                                div()
                                    .w(px(24.))
                                    .h(px(22.))
                                    .rounded(px(5.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(if has_tag_filter {
                                        theme.accent_overlay()
                                    } else {
                                        rgba(0x00000000)
                                    })
                                    .cursor(CursorStyle::PointingHand)
                                    .on_mouse_down(MouseButton::Left, {
                                        let this = this.clone();
                                        move |_ev, _window, cx| {
                                            let _ = this.update(cx, |bar, cx| {
                                                bar.tag_panel_open = !bar.tag_panel_open;
                                                cx.notify();
                                            });
                                        }
                                    })
                                    .child(
                                        div()
                                            .text_size(px(14.))
                                            .font_family("iconfont")
                                            .text_color(if has_tag_filter { accent } else { text_2 })
                                            .child("\u{ec07}"),
                                    ),
                            ),
                    ),
            )
    }
}
