//! Filter bar — content type filter buttons + tag filter button.
//!
//! Split out of the legacy combined `SearchBar`: this component owns the type
//! and tag filter UI plus their floating-panel open states. Keyword input
//! lives in `SearchBox`.

use crate::ui::font::fs;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui::{InteractiveElement, StatefulInteractiveElement};
use gpui_component::tooltip::Tooltip;

use crate::core::frontend::PANEL_OFFSET_X;
use crate::core::i18n_keys::I18nKey;
use crate::state::app::AppState;

use super::clipboard_list::ClipboardListView;
use super::components::shortcut_hint::external_shortcut;
use super::theme::ClippiTheme;

pub(crate) struct FilterDef {
    pub key: &'static str,
    pub icon: &'static str,
    pub label_key: I18nKey,
}

pub(crate) const FILTER_TYPES: &[FilterDef] = &[
    FilterDef {
        label_key: I18nKey::FilterTextLabel,
        key: "plain_text",
        icon: "\u{e606}",
    },
    FilterDef {
        label_key: I18nKey::FilterRtfLabel,
        key: "rich_text",
        icon: "\u{e853}",
    },
    FilterDef {
        label_key: I18nKey::FilterImageLabel,
        key: "image",
        icon: "\u{e626}",
    },
    FilterDef {
        label_key: I18nKey::FilterFilesLabel,
        key: "file",
        icon: "\u{e68a}",
    },
    FilterDef {
        label_key: I18nKey::FilterLinksLabel,
        key: "link",
        icon: "\u{e6d7}",
    },
    FilterDef {
        label_key: I18nKey::FilterPathLabel,
        key: "path",
        icon: "\u{e60f}",
    },
    FilterDef {
        label_key: I18nKey::FilterColorLabel,
        key: "color",
        icon: "\u{e608}",
    },
    FilterDef {
        label_key: I18nKey::FilterContactLabel,
        key: "contact",
        icon: "\u{e616}",
    },
];
const SEARCH_BAR_HORIZONTAL_PADDING: f32 = 16.0;

/// Look up the display icon and label for a built-in filter type key.
pub(crate) fn filter_type_display(key: &str) -> Option<(&'static str, String)> {
    FILTER_TYPES
        .iter()
        .find(|f| f.key == key)
        .map(|f| (f.icon, f.label_key.text().to_string()))
}
const TOOLBAR_GROUP_GAP: f32 = 6.0;
const TOOLBAR_DIVIDER_WIDTH: f32 = 1.0;
const TAG_FILTER_BUTTON_WIDTH: f32 = 24.0;
const TYPE_FILTER_TEXT_GAP: f32 = 4.0;
const TYPE_FILTER_ICON_GAP: f32 = 3.0;

/// Rendering and Alt dispatch must use this exact same ordered set.
pub(crate) fn visible_filter_types(
    config: &[crate::core::settings::TypeFilterEntry],
) -> Vec<&'static FilterDef> {
    config
        .iter()
        .filter(|entry| entry.visible)
        .filter_map(|entry| FILTER_TYPES.iter().find(|filter| filter.key == entry.key))
        .collect()
}

fn filter_label_width(label: &'static str, window: &Window) -> f32 {
    let text: SharedString = label.into();
    let mut style = window.text_style();
    style.font_weight = FontWeight::BOLD;
    let run = style.to_run(text.len());
    f32::from(
        window
            .text_system()
            .shape_line(text, fs(11.), &[run], None)
            .width,
    )
}

/// Shared toolbar geometry for clipboard types and picker categories.
pub(crate) fn filter_toolbar() -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_start()
        .gap(px(TOOLBAR_GROUP_GAP))
        .h(px(22.))
        .w_full()
        .px(px(8.))
        .pb(px(2.))
        .flex_shrink_0()
}

/// Keep picker choices visually identical to the existing clipboard type chips.
pub(crate) fn filter_chip(
    id: impl Into<ElementId>,
    active: bool,
    theme: &ClippiTheme,
) -> Stateful<Div> {
    let inactive = if theme.bg == rgb(0x191a1b) {
        rgba(0xffffff0a)
    } else {
        rgba(0x00000008)
    };
    div()
        .id(id)
        .h(px(22.))
        .flex_1()
        .min_w(px(0.))
        .flex_shrink_0()
        .rounded(px(5.))
        .bg(if active {
            theme.accent_overlay()
        } else {
            inactive
        })
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::PointingHand)
}

pub struct FilterBar {
    state: Entity<AppState>,
    list_view: Entity<ClipboardListView>,
    tag_panel_open: bool,
    filter_config_open: bool,
    alt_shortcuts_available: bool,
    theme: ClippiTheme,
}

impl FilterBar {
    pub fn new(
        state: Entity<AppState>,
        list_view: Entity<ClipboardListView>,
        theme: ClippiTheme,
    ) -> Self {
        Self {
            state,
            list_view,
            tag_panel_open: false,
            filter_config_open: false,
            alt_shortcuts_available: false,
            theme,
        }
    }

    pub fn tag_panel_open(&self) -> bool {
        self.tag_panel_open
    }

    pub fn close_tag_panel(&mut self, cx: &mut Context<Self>) {
        self.tag_panel_open = false;
        cx.notify();
    }

    pub fn filter_config_open(&self) -> bool {
        self.filter_config_open
    }

    pub fn close_filter_config(&mut self, cx: &mut Context<Self>) {
        self.filter_config_open = false;
        cx.notify();
    }

    pub fn set_alt_shortcuts_available(&mut self, available: bool, cx: &mut Context<Self>) {
        if self.alt_shortcuts_available != available {
            self.alt_shortcuts_available = available;
            cx.notify();
        }
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
            state.visible_items()
        });
        list_view.update(cx, |list, cx| list.set_items(items, cx));
    }
}

impl Render for FilterBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        let accent = theme.accent;
        let text_2 = theme.text_2;
        let this = cx.entity().clone();
        let state_snapshot = self.state.read(cx);
        let alt_pressed = self.alt_shortcuts_available
            && state_snapshot.alt_pressed
            && state_snapshot.settings.alt_shortcuts_enabled;
        let has_tag_filter = !state_snapshot.filters.tag_ids.is_empty();
        let viewport = window.viewport_size();
        let toolbar_width =
            f32::from(viewport.width) - PANEL_OFFSET_X - SEARCH_BAR_HORIZONTAL_PADDING;
        let visible_entries = visible_filter_types(&state_snapshot.settings.type_filter_config);
        let active_types: Vec<bool> = visible_entries
            .iter()
            .map(|filter| state_snapshot.filters.is_type_active(filter.key))
            .collect();
        let visible_count = visible_entries.len() as f32;
        let type_toolbar_width = (toolbar_width
            - (TOOLBAR_GROUP_GAP * 2.0)
            - TOOLBAR_DIVIDER_WIDTH
            - TAG_FILTER_BUTTON_WIDTH)
            .max(0.0);
        let text_gap_total = TYPE_FILTER_TEXT_GAP * (visible_count - 1.0).max(0.0);
        let type_slot_width =
            (type_toolbar_width - text_gap_total).max(0.0) / visible_count.max(1.0);
        // Measure the actual translated label in the selected font. Alt never
        // participates in this decision: labels and gaps stay stable on press.
        let label_width = visible_entries
            .iter()
            .map(|filter| filter_label_width(filter.label_key.text(), window))
            .fold(0.0_f32, f32::max);

        let text_slot_width = f32::from(fs(12.)) + 2.0 + label_width + 10.0;
        let icon_only = type_slot_width < text_slot_width;
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

        filter_toolbar()
            .on_mouse_down(MouseButton::Right, {
                let this = this.clone();
                move |_ev, _window, cx| {
                    cx.stop_propagation();
                    this.update(cx, |bar, cx| {
                        bar.filter_config_open = !bar.filter_config_open;
                        cx.notify();
                    });
                }
            })
            .when(!visible_entries.is_empty(), |toolbar| {
                toolbar.child(
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
                        .children(
                            visible_entries
                                .iter()
                                .enumerate()
                                .map(|(index, filter_def)| {
                                    let icon = filter_def.icon;
                                    let label = filter_def.label_key.text();
                                    let key = filter_def.key;
                                    let is_active = active_types[index];
                                    let filter_text = if is_active { accent } else { text_2 };
                                    let filter_weight = if is_active {
                                        FontWeight::BOLD
                                    } else {
                                        FontWeight::default()
                                    };
                                    let state = self.state.clone();
                                    let list_view = self.list_view.clone();
                                    let this = this.clone();

                                    let content = div()
                                        .flex()
                                        .flex_shrink_0()
                                        .items_center()
                                        .gap(px(2.))
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_size(fs(12.))
                                                .font_family("iconfont")
                                                .text_color(filter_text)
                                                .child(icon.to_string()),
                                        )
                                        .when(!icon_only, |group| {
                                            group.child(
                                                div()
                                                    .text_size(fs(11.))
                                                    .font_weight(filter_weight)
                                                    .text_color(filter_text)
                                                    .child(label),
                                            )
                                        });

                                    let button =
                                        filter_chip(("filter-type", index), is_active, theme)
                                            .overflow_hidden()
                                            .when(icon_only, move |button| {
                                                button.tooltip(move |window, cx| {
                                                    Tooltip::element(move |_window, _cx| {
                                                        div().text_size(fs(10.)).child(label)
                                                    })
                                                    .build(window, cx)
                                                })
                                            })
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                move |_ev, _window, cx| {
                                                    Self::apply_type_filter(
                                                        &state, &list_view, key, cx,
                                                    );
                                                    this.update(cx, |_bar, cx| cx.notify());
                                                },
                                            )
                                            .child(content);
                                    external_shortcut(
                                        SharedString::from(format!(
                                            "filter-shortcut-external-{key}"
                                        )),
                                        (index + 1).to_string(),
                                        alt_pressed && index < 9,
                                        button,
                                        theme,
                                        window,
                                        cx,
                                    )
                                }),
                        ),
                )
            })
            .when(visible_entries.is_empty(), |toolbar| {
                toolbar.child(div().flex_1())
            })
            .when(!visible_entries.is_empty(), |toolbar| {
                toolbar.child(
                    div()
                        .w(px(TOOLBAR_DIVIDER_WIDTH))
                        .h(px(14.))
                        .flex_shrink_0()
                        .bg(toolbar_divider),
                )
            })
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
                            .tooltip(|window, cx| {
                                let label = I18nKey::FilterTagsTooltip.text();
                                Tooltip::element(move |_window, _cx| {
                                    div().text_size(fs(10.)).child(label)
                                })
                                .build(window, cx)
                            })
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
                                    .text_size(fs(14.))
                                    .font_family("iconfont")
                                    .text_color(if has_tag_filter { accent } else { text_2 })
                                    .child("\u{e886}"),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::visible_filter_types;
    use crate::core::settings::TypeFilterEntry;

    #[test]
    fn hidden_and_unknown_types_do_not_consume_shortcut_positions() {
        let config: Vec<_> = [
            ("plain_text", true),
            ("rich_text", false),
            ("obsolete", true),
            ("image", false),
            ("file", true),
            ("link", true),
        ]
        .into_iter()
        .map(|(key, visible)| TypeFilterEntry {
            key: key.into(),
            visible,
        })
        .collect();
        let keys: Vec<_> = visible_filter_types(&config)
            .iter()
            .map(|filter| filter.key)
            .collect();
        assert_eq!(keys, ["plain_text", "file", "link"]);
    }

    #[test]
    fn reordered_single_and_empty_type_lists_keep_their_visible_order() {
        let mut config: Vec<_> = ["link", "plain_text", "file"]
            .into_iter()
            .map(|key| TypeFilterEntry {
                key: key.into(),
                visible: true,
            })
            .collect();
        assert_eq!(
            visible_filter_types(&config)
                .iter()
                .map(|filter| filter.key)
                .collect::<Vec<_>>(),
            ["link", "plain_text", "file"]
        );
        config[0].visible = false;
        config[1].visible = false;
        assert_eq!(visible_filter_types(&config)[0].key, "file");
        config[2].visible = false;
        assert!(visible_filter_types(&config).is_empty());
    }
}
