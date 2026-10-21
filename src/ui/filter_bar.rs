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

/// Identical trailing action slot for the type and source toolbars.
pub(crate) fn filter_action_slot() -> Div {
    div()
        .w(px(TAG_FILTER_BUTTON_WIDTH))
        .min_w(px(TAG_FILTER_BUTTON_WIDTH))
        .max_w(px(TAG_FILTER_BUTTON_WIDTH))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
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

/// Always reserve at least two slots: all sources and an application choice.
pub(crate) fn source_app_chip_width(width: f32, visible_types: usize, gap: f32) -> f32 {
    let columns = visible_types.max(2) as f32;
    let available = (width
        - SEARCH_BAR_HORIZONTAL_PADDING
        - TAG_FILTER_BUTTON_WIDTH
        - TOOLBAR_DIVIDER_WIDTH
        - TOOLBAR_GROUP_GAP * 2.)
        .max(0.);
    (available - gap * (columns - 1.)).max(0.) / columns
}

/// Source choices and interactions shared by the main and nonactivating quick window.
pub(crate) fn source_app_filter_bar(
    snapshot: &AppState,
    filters: &crate::core::filters::ClipboardFilters,
    theme: &ClippiTheme,
    scroll: &ScrollHandle,
    owner: EntityId,
    layout: super::components::scroll_filter_bar::ScrollFilterLayout<'_>,
    on_select: impl Fn(Option<&str>, &mut App) + 'static,
) -> Div {
    let chip_width = layout.chip_width;
    let on_select = std::rc::Rc::new(on_select);
    let mut choices = vec![None];
    choices.extend(snapshot.source_app_names.iter().cloned().map(Some));
    let mut choices_ui = Vec::new();
    for (index, name) in choices.into_iter().enumerate() {
        let active = name
            .as_deref()
            .map_or(!filters.has_source_app_filter(), |name| {
                filters.is_source_app_active(name)
            });
        let label = name
            .clone()
            .unwrap_or_else(|| I18nKey::SourceAppFilterAll.text().to_string());
        let content = if let Some(path) = name
            .as_ref()
            .and_then(|name| snapshot.source_app_icons.get(name))
        {
            img(path.clone()).size(px(16.)).into_any_element()
        } else if name.is_none() {
            svg()
                .path("picker-icons/layout-grid.svg")
                .size(fs(12.))
                .text_color(if active { theme.accent } else { theme.text_2 })
                .into_any_element()
        } else {
            // Apps without an icon keep the same fixed target size.
            div()
                .font_family("iconfont")
                .text_size(fs(14.))
                .child("\u{e616}")
                .into_any_element()
        };
        let tooltip = label.clone();
        let on_select = on_select.clone();
        let scroll = scroll.clone();
        choices_ui.push(
            super::components::scroll_filter_bar::scroll_filter_chip(
                ("source-app", index),
                active,
                theme,
                chip_width,
            )
            .overflow_hidden()
            .text_size(fs(11.))
            .text_color(if active { theme.accent } else { theme.text_2 })
            .tooltip(move |window, cx| {
                let label = tooltip.clone();
                Tooltip::element(move |_, _| div().text_size(fs(10.)).child(label.clone()))
                    .build(window, cx)
            })
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
                if name.is_none() {
                    scroll.set_offset(Point::default());
                }
                on_select(name.as_deref(), cx);
            })
            .child(content),
        );
    }
    super::components::scroll_filter_bar::scroll_filter_bar(
        "source-apps",
        theme,
        scroll,
        owner,
        choices_ui,
        layout,
    )
}

pub struct FilterBar {
    state: Entity<AppState>,
    list_view: Entity<ClipboardListView>,
    tag_panel_open: bool,
    filter_config_open: bool,
    alt_shortcuts_available: bool,
    theme: ClippiTheme,
    source_scroll: ScrollHandle,
    measured_width: std::rc::Rc<std::cell::Cell<f32>>,
    source_scroll_left: std::cell::Cell<bool>,
}

impl FilterBar {
    pub fn new(
        state: Entity<AppState>,
        list_view: Entity<ClipboardListView>,
        theme: ClippiTheme,
    ) -> Self {
        Self {
            source_scroll: ScrollHandle::new(),
            measured_width: Default::default(),
            source_scroll_left: std::cell::Cell::new(false),
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

    pub fn reset_source_scroll(&mut self, cx: &mut Context<Self>) {
        self.source_scroll.set_offset(Point::default());
        self.source_scroll_left.set(false);
        cx.notify();
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

    fn source_bar(&self, chip_width: f32, chip_gap: f32, cx: &mut Context<Self>) -> Div {
        let state = self.state.clone();
        let list = self.list_view.clone();
        source_app_filter_bar(
            self.state.read(cx),
            &self.state.read(cx).filters,
            &self.theme,
            &self.source_scroll,
            cx.entity().entity_id(),
            super::components::scroll_filter_bar::ScrollFilterLayout {
                chip_width,
                chip_gap,
                scroll_left: &self.source_scroll_left,
            },
            move |name, cx| {
                let items = state.update(cx, |state, cx| {
                    state.set_source_app_filter(name);
                    cx.notify();
                    state.visible_items()
                });
                list.update(cx, |list, cx| list.set_items(items, cx));
            },
        )
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
        let source_enabled = state_snapshot.settings.show_source_app_filter();
        let has_tag_filter = !state_snapshot.filters.tag_ids.is_empty();
        let viewport = window.viewport_size();
        let measured_width = self.measured_width.get();
        let toolbar_width = if measured_width > 0. {
            measured_width - SEARCH_BAR_HORIZONTAL_PADDING
        } else {
            f32::from(viewport.width) - PANEL_OFFSET_X - SEARCH_BAR_HORIZONTAL_PADDING
        };
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
        let chip_gap = if icon_only {
            TYPE_FILTER_ICON_GAP
        } else {
            TYPE_FILTER_TEXT_GAP
        };
        let chip_width = source_app_chip_width(
            toolbar_width + SEARCH_BAR_HORIZONTAL_PADDING,
            visible_entries.len(),
            chip_gap,
        );

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

        let type_bar =
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
                            .children(visible_entries.iter().enumerate().map(
                                |(index, filter_def)| {
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
                                            .border_1()
                                            .border_color(if is_active {
                                                Rgba {
                                                    a: 0.45,
                                                    ..theme.accent
                                                }
                                            } else {
                                                rgba(0x00000000)
                                            })
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
                                },
                            )),
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
                    filter_action_slot().child(
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
                );
        let measured_width = self.measured_width.clone();
        let owner = cx.entity().entity_id();
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w_full()
            .gap(px(4.))
            .child(type_bar)
            .when(source_enabled, |bar| {
                bar.child(self.source_bar(chip_width, chip_gap, cx))
            })
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, _, cx| {
                        let width = f32::from(bounds.size.width);
                        if (measured_width.get() - width).abs() > 0.1 {
                            measured_width.set(width);
                            cx.notify(owner);
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::visible_filter_types;
    use crate::core::settings::TypeFilterEntry;

    #[test]
    fn source_bar_leaves_room_for_an_app_when_types_are_hidden() {
        for width in [324., 430.] {
            for types in [0, 1] {
                let slot = super::source_app_chip_width(width, types, 4.);
                assert!(slot >= 24.);
                assert!(
                    slot < width / 2.,
                    "the pinned all-sources chip must not fill the row"
                );
            }
        }
    }

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
