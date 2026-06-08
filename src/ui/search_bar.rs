//! Search bar - keyword input + content type filter buttons.
//!
//! --- Matches the original Slint ClipboardList.slint design: ---
//! --- - 28px search box, 10px border-radius, surface bg, divider border ---
//! --- - Filter buttons: 22px height, 5px border-radius, icon+label layout ---
//! --- - Active state: accent text + 22% accent bg overlay ---
//! --- - Type/tag toolbar switches to icon-only when the window is narrow ---

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui::{InteractiveElement, StatefulInteractiveElement};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::tooltip::Tooltip;

use crate::core::frontend::PANEL_OFFSET_X;
use crate::core::i18n_keys::I18nKey;
use crate::state::app::AppState;

use super::clipboard_list::ClipboardListView;
use super::theme::ClippiTheme;

struct FilterDef {
    key: &'static str,
    icon: &'static str,
    label_key: I18nKey,
}

const FILTER_TYPES: &[FilterDef] = &[
    FilterDef {
        label_key: I18nKey::FilterTextLabel,
        key: "plain_text",
        icon: "\u{e60e}",
    },
    FilterDef {
        label_key: I18nKey::FilterRtfLabel,
        key: "rich_text",
        icon: "\u{e6ae}",
    },
    FilterDef {
        label_key: I18nKey::FilterFilesLabel,
        key: "file",
        icon: "\u{e646}",
    },
    FilterDef {
        label_key: I18nKey::FilterLinksLabel,
        key: "link",
        icon: "\u{e6d7}",
    },
    FilterDef {
        label_key: I18nKey::FilterColorLabel,
        key: "color",
        icon: "\u{e610}",
    },
];
const SEARCH_BAR_HORIZONTAL_PADDING: f32 = 16.0;
const TOOLBAR_GROUP_GAP: f32 = 6.0;
const TOOLBAR_DIVIDER_WIDTH: f32 = 1.0;
const TAG_FILTER_BUTTON_WIDTH: f32 = 24.0;
const TYPE_FILTER_TEXT_GAP: f32 = 4.0;
const TYPE_FILTER_ICON_GAP: f32 = 3.0;
const TYPE_FILTER_TEXT_MIN_SLOT_WIDTH: f32 = 50.0;

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
        theme: ClippiTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(I18nKey::SearchPlaceholderFull.text()));
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
            theme,
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

    pub fn set_theme(&mut self, theme: ClippiTheme, cx: &mut Context<Self>) {
        self.theme = theme;
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        let accent = theme.accent;
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;
        let surface = theme.surface;
        let divider = theme.divider;
        let this = cx.entity().clone();
        let state_snapshot = self.state.read(cx);
        let has_tag_filter = !state_snapshot.filters.tag_ids.is_empty();
        let viewport = window.viewport_size();
        let toolbar_width =
            f32::from(viewport.width) - PANEL_OFFSET_X - SEARCH_BAR_HORIZONTAL_PADDING;
        let type_count = FILTER_TYPES.len() as f32;
        let type_toolbar_width = (toolbar_width
            - (TOOLBAR_GROUP_GAP * 2.0)
            - TOOLBAR_DIVIDER_WIDTH
            - TAG_FILTER_BUTTON_WIDTH)
            .max(0.0);
        let text_gap_total = TYPE_FILTER_TEXT_GAP * (type_count - 1.0).max(0.0);
        let type_slot_width = (type_toolbar_width - text_gap_total).max(0.0) / type_count.max(1.0);
        let icon_only = type_slot_width < TYPE_FILTER_TEXT_MIN_SLOT_WIDTH;
        let inactive_button_bg = if theme.bg == rgb(0x191a1b) {
            rgba(0xffffff0a)
        } else {
            rgba(0x00000008)
        };
        let toolbar_divider = if theme.bg == rgb(0x191a1b) {
            rgba(0xffffff18)
        } else {
            rgba(0x00000014)
        };

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
                    .justify_start()
                    .gap(px(TOOLBAR_GROUP_GAP))
                    .h(px(22.))
                    .w_full()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .min_w(px(0.))
                            .items_center()
                            .gap(if icon_only {
                                px(TYPE_FILTER_ICON_GAP)
                            } else {
                                px(TYPE_FILTER_TEXT_GAP)
                            })
                            .children(FILTER_TYPES.iter().enumerate().map(|(index, f)| {
                                let is_active = if f.key == "file" {
                                    state_snapshot.filters.is_type_active("file")
                                        || state_snapshot.filters.is_type_active("image")
                                } else {
                                    state_snapshot.filters.is_type_active(f.key)
                                };
                                let filter_bg = if is_active {
                                    theme.accent_overlay()
                                } else {
                                    inactive_button_bg
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
                                let label = f.label_key.text();

                                div()
                                    .id(("filter-type", index))
                                    .h(px(22.))
                                    .when(icon_only, |button| {
                                        button.flex_1().min_w(px(0.)).justify_center()
                                    })
                                    .when(!icon_only, |button| {
                                        button
                                            .flex_1()
                                            .min_w(px(0.))
                                            .justify_center()
                                            .px(px(5.))
                                            .gap(px(2.))
                                    })
                                    .flex_shrink_0()
                                    .rounded(px(5.))
                                    .bg(filter_bg)
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .when(icon_only, move |button| {
                                        let label_for_tip = label;
                                        button.tooltip(move |window, cx| {
                                            Tooltip::new(label_for_tip).build(window, cx)
                                        })
                                    })
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                        Self::apply_type_filter(&state, &list_view, key, cx);
                                        this.update(cx, |_bar, cx| cx.notify());
                                    })
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_family("iconfont")
                                            .text_color(filter_text)
                                            .child(f.icon.to_string()),
                                    )
                                    .when(!icon_only, |button| {
                                        button.child(
                                            div()
                                                .text_size(px(11.))
                                                .font_weight(filter_weight)
                                                .text_color(filter_text)
                                                .child(label),
                                        )
                                    })
                            })),
                    )
                    .child(
                        div()
                            .w(px(TOOLBAR_DIVIDER_WIDTH))
                            .h(px(14.))
                            .flex_shrink_0()
                            .bg(toolbar_divider),
                    )
                    .child(
                        div()
                            .w(px(TAG_FILTER_BUTTON_WIDTH))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .id("filter-tags")
                                    .w(px(24.))
                                    .h(px(22.))
                                    .flex_shrink_0()
                                    .rounded(px(5.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .tooltip(|window, cx| Tooltip::new(I18nKey::FilterTagsTooltip.text()).build(window, cx))
                                    .bg(if has_tag_filter {
                                        theme.accent_overlay()
                                    } else {
                                        inactive_button_bg
                                    })
                                    .cursor(CursorStyle::PointingHand)
                                    .on_mouse_down(MouseButton::Left, {
                                        let this = this.clone();
                                        move |_ev, _window, cx| {
                                            this.update(cx, |bar, cx| {
                                                bar.tag_panel_open = !bar.tag_panel_open;
                                                cx.notify();
                                            });
                                        }
                                    })
                                    .child(
                                        div()
                                            .text_size(px(14.))
                                            .font_family("iconfont")
                                            .text_color(if has_tag_filter {
                                                accent
                                            } else {
                                                text_2
                                            })
                                            .child("\u{ec07}"),
                                    ),
                            ),
                    ),
            )
    }
}
