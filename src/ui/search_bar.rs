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

use super::clipboard_list::{ClipboardListEvent, ClipboardListView};
use super::theme::ClippiTheme;

fn primary_modifier_pressed(modifiers: Modifiers) -> bool {
    modifiers.secondary()
}

pub(crate) struct FilterDef {
    pub key: &'static str,
    pub icon: &'static str,
    pub label_key: I18nKey,
}

pub(crate) const FILTER_TYPES: &[FilterDef] = &[
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
        icon: "\u{e610}",
    },
    FilterDef {
        label_key: I18nKey::FilterContactLabel,
        key: "contact",
        icon: "\u{e604}",
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
const TYPE_FILTER_TEXT_MIN_SLOT_WIDTH: f32 = 50.0;

pub struct SearchBar {
    input: Entity<InputState>,
    state: Entity<AppState>,
    list_view: Entity<ClipboardListView>,
    tag_panel_open: bool,
    filter_config_open: bool,
    theme: ClippiTheme,
    last_lang_version: u64,
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
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(I18nKey::SearchPlaceholderFull.text())
        });
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
            filter_config_open: false,
            theme,
            last_lang_version: crate::core::i18n::lang_version(),
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

    pub fn filter_config_open(&self) -> bool {
        self.filter_config_open
    }

    pub fn close_filter_config(&mut self, cx: &mut Context<Self>) {
        self.filter_config_open = false;
        cx.notify();
    }

    pub fn set_theme(&mut self, theme: ClippiTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// Focus the search input. Called when the window opens and
    /// `auto_focus_search` setting is enabled.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.focus_handle(cx).focus(window);
    }

    /// Clear the search input text. Called when the window opens and
    /// clear_search_on_show setting is enabled.
    pub fn clear_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
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
        // 语言切换时刷新 InputState.placeholder
        let current = crate::core::i18n::lang_version();
        if self.last_lang_version != current {
            self.last_lang_version = current;
            self.input.update(cx, |state, cx| {
                state.set_placeholder(I18nKey::SearchPlaceholderFull.text(), window, cx);
            });
        }

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
        let visible_entries: Vec<&crate::core::settings::TypeFilterEntry> = state_snapshot
            .settings
            .type_filter_config
            .iter()
            .filter(|e| e.visible)
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
                    .on_key_down({
                        let list = self.list_view.clone();
                        let app_state = self.state.clone();
                        move |ev: &KeyDownEvent, window, cx| {
                            let key = ev.keystroke.key.as_str();
                            let ctrl = primary_modifier_pressed(ev.keystroke.modifiers);
                            let shift = ev.keystroke.modifiers.shift;

                            // --- Navigation: up/down — keep search focus, move list selection ---
                            if !ctrl && !shift {
                                match key {
                                    "up" => {
                                        list.update(cx, |list, cx| {
                                            list.select_previous(gpui::ScrollStrategy::Top, cx);
                                        });
                                        cx.stop_propagation();
                                        return;
                                    }
                                    "down" => {
                                        list.update(cx, |list, cx| {
                                            list.select_next(gpui::ScrollStrategy::Bottom, cx);
                                        });
                                        cx.stop_propagation();
                                        return;
                                    }
                                    "escape" => {
                                        list.update(cx, |list, cx| {
                                            if !list.handle_escape(cx) {
                                                cx.emit(ClipboardListEvent::RequestHide);
                                            }
                                        });
                                        cx.stop_propagation();
                                        return;
                                    }
                                    _ => {}
                                }
                            }

                            // --- Action shortcuts: focus list + execute action ---
                            match (ctrl, shift, key) {
                                // Enter — paste with plain setting
                                (false, false, "enter") => {
                                    let plain = app_state.read(cx).settings.copy_as_plain_text;
                                    list.update(cx, |list, cx| {
                                        list.focus(window);
                                        list.action_paste(plain, cx);
                                    });
                                    cx.stop_propagation();
                                }
                                // Shift+Enter — paste as plain text
                                (false, true, "enter") => {
                                    list.update(cx, |list, cx| {
                                        list.focus(window);
                                        list.action_paste(true, cx);
                                    });
                                    cx.stop_propagation();
                                }
                                // Ctrl+D — toggle favorite
                                (true, false, "d") => {
                                    list.update(cx, |list, cx| {
                                        list.focus(window);
                                        list.action_toggle_favorite(cx);
                                    });
                                    cx.stop_propagation();
                                }
                                // Ctrl+E — edit
                                (true, false, "e") => {
                                    list.update(cx, |list, cx| {
                                        list.focus(window);
                                        list.action_edit(cx);
                                    });
                                    cx.stop_propagation();
                                }
                                // F2 — edit note
                                (false, false, "f2") => {
                                    list.update(cx, |list, cx| {
                                        list.focus(window);
                                        list.action_edit_note(window, cx);
                                    });
                                    cx.stop_propagation();
                                }
                                // Delete — delete item(s)
                                (false, false, "delete") => {
                                    list.update(cx, |list, cx| {
                                        list.focus(window);
                                        list.action_delete(cx);
                                    });
                                    cx.stop_propagation();
                                }
                                _ => {}
                            }
                        }
                    })
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
                            .children(visible_entries.iter().enumerate().filter_map(
                                |(index, entry)| {
                                    // Look up from FILTER_TYPES; unknown keys are silently skipped
                                    let filter_def =
                                        FILTER_TYPES.iter().find(|fd| fd.key == entry.key)?;
                                    let icon = filter_def.icon;
                                    let label = filter_def.label_key.text();
                                    let key = filter_def.key;
                                    let is_active = state_snapshot.filters.is_type_active(key);
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

                                    Some(
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
                                                button.tooltip(move |window, cx| {
                                                    Tooltip::element(move |_window, _cx| {
                                                        div().text_size(px(10.)).child(label)
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
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .font_family("iconfont")
                                                    .text_color(filter_text)
                                                    .child(icon.to_string()),
                                            )
                                            .when(!icon_only, |button| {
                                                button.child(
                                                    div()
                                                        .text_size(px(11.))
                                                        .font_weight(filter_weight)
                                                        .text_color(filter_text)
                                                        .child(label),
                                                )
                                            }),
                                    )
                                },
                            )),
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
                                    .tooltip(|window, cx| {
                                        let label = I18nKey::FilterTagsTooltip.text();
                                        Tooltip::element(move |_window, _cx| {
                                            div().text_size(px(10.)).child(label)
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
