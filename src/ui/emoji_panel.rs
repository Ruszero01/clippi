//! Offline emoji, kaomoji and symbol picker. The foreground-app bar belongs to RootView.
use gpui::prelude::*;
use gpui::*;
use gpui_component::input::{InputEvent, InputState};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use gpui_component::tooltip::Tooltip;

use super::filter_bar::{filter_chip, filter_toolbar};
use super::font::fs;
use super::theme::ClippiTheme;
use crate::core::emoji::{self, EmojiEntry};
use crate::core::i18n_keys::I18nKey;
use crate::core::{kaomoji, symbols};
use crate::state::app::AppState;

const EMOJI_CATEGORIES: &[(&str, &str, &str)] = &[
    ("最近使用", "Recent", "clock"),
    ("笑脸与情感", "Smileys & emotion", "smile"),
    ("人物与手势", "People & body", "hand"),
    ("动物与自然", "Animals & nature", "leaf"),
    ("食物与饮料", "Food & drink", "apple"),
    ("旅行与地点", "Travel & places", "plane"),
    ("活动", "Activities", "trophy"),
    ("物品", "Objects", "lightbulb"),
    ("符号", "Symbols", "hash"),
    ("旗帜", "Flags", "flag"),
];

// The recent row can be shorter than the regular grid and has a heading below it.
fn vertical_selection(
    index: usize,
    down: bool,
    columns: usize,
    recent: usize,
    len: usize,
) -> usize {
    if len == 0 {
        return 0;
    }
    let next = if recent > 0 && index < recent {
        if down && recent < len {
            recent + index
        } else {
            index
        }
    } else if recent > 0 && index < recent + columns && !down {
        (index - recent).min(recent - 1)
    } else if down {
        index.saturating_add(columns)
    } else {
        index.saturating_sub(columns)
    };
    next.min(len - 1)
}

pub enum EmojiPanelEvent {
    Back,
}
impl EventEmitter<EmojiPanelEvent> for EmojiPanel {}

pub struct EmojiPanel {
    state: Entity<AppState>,
    input: Entity<InputState>,
    focus_handle: FocusHandle,
    kind: usize,
    category: usize,
    query: String,
    entries: Vec<&'static EmojiEntry>,
    selected: usize,
    recent_len: usize,
    columns: usize,
    scroll: UniformListScrollHandle,
    category_scroll: ScrollHandle,
    category_scroll_left: std::cell::Cell<bool>,
    lang_version: u64,
    _subscription: Subscription,
}

impl EmojiPanel {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(I18nKey::EmojiSearch.text()));
        let subscription = cx.subscribe(&input, |this, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.query = input.read(cx).value().to_lowercase();
                this.rebuild(cx);
            }
        });
        let mut panel = Self {
            state,
            input,
            focus_handle: cx.focus_handle(),
            kind: 0,
            category: 1,
            query: String::new(),
            entries: Vec::new(),
            selected: 0,
            recent_len: 0,
            columns: 8,
            scroll: UniformListScrollHandle::new(),
            category_scroll: ScrollHandle::new(),
            category_scroll_left: std::cell::Cell::new(false),
            lang_version: crate::core::i18n::lang_version(),
            _subscription: subscription,
        };
        panel.rebuild(cx);
        panel
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.focus_handle(cx).focus(window);
    }

    pub fn input_entity(&self) -> Entity<InputState> {
        self.input.clone()
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.rebuild(cx);
    }

    fn categories(&self) -> Vec<(&'static str, &'static str, &'static str)> {
        match self.kind {
            1 => std::iter::once(EMOJI_CATEGORIES[0])
                .chain(kaomoji::CATEGORIES.iter().copied())
                .collect(),
            2 => std::iter::once(EMOJI_CATEGORIES[0])
                .chain(symbols::CATEGORIES.iter().copied())
                .collect(),
            _ => EMOJI_CATEGORIES.to_vec(),
        }
    }

    fn category_label(&self) -> &'static str {
        let item = self.categories()[self.category];
        if crate::core::i18n::is_en() {
            item.1
        } else {
            item.0
        }
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let source = match self.kind {
            0 => emoji::emojis(),
            1 => emoji::extra_entries(true),
            _ => emoji::extra_entries(false),
        };
        self.entries.clear();
        self.recent_len = 0;
        let searching = !self.query.trim().is_empty();
        if !searching && (self.category == 0 || (self.kind == 0 && self.category == 1)) {
            let settings = &self.state.read(cx).settings;
            let recent = match self.kind {
                1 => &settings.recent_kaomojis,
                2 => &settings.recent_symbols,
                _ => &settings.recent_emojis,
            };
            for text in recent {
                if let Some(entry) = source.iter().find(|entry| entry.text == *text) {
                    if !self.entries.iter().any(|old| old.text == entry.text) {
                        self.entries.push(entry);
                    }
                }
            }
            if self.category == 1 {
                self.entries.truncate(self.columns);
            }
            self.recent_len = self.entries.len();
        }
        if searching || self.category != 0 {
            self.entries.extend(source.iter().filter(|entry| {
                if searching {
                    entry.matches(&self.query)
                } else if self.kind == 2 {
                    symbols::in_category(entry, self.category - 1)
                } else if self.kind == 1 {
                    kaomoji::in_category(entry, self.category - 1)
                } else {
                    !entry.variant && entry.group == emoji::PICKER_GROUPS[self.category - 1]
                }
            }));
        }
        if self.kind == 2 && self.category == 1 && !searching {
            self.entries.sort_by_key(|entry| {
                symbols::COMMON
                    .find(entry.text.as_str())
                    .unwrap_or(usize::MAX)
            });
        }
        self.selected = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn search_placeholder(&self) -> &'static str {
        match self.kind {
            1 => I18nKey::EmojiSearchKaomoji.text(),
            2 => I18nKey::EmojiSearchSymbols.text(),
            _ => I18nKey::EmojiSearch.text(),
        }
    }

    fn paste(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        self.selected = index;
        self.state.update(cx, |state, cx| {
            state.paste_picker_text(&entry.text, self.kind);
            cx.notify();
        });
        cx.notify();
    }

    fn choose_category(&mut self, category: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.category = category;
        // The first category is pinned outside the shared scrolling viewport.
        if category > 0 {
            let start = (category - 1) as f32 * 35.;
            let offset = -f32::from(self.category_scroll.offset().x);
            let width = f32::from(self.category_scroll.bounds().size.width);
            let limit = f32::from(self.category_scroll.max_offset().width);
            let target = if start < offset {
                start
            } else if start + 32. > offset + width {
                ((start + 32. - width) / 35.).ceil() * 35.
            } else {
                offset
            };
            self.category_scroll
                .set_offset(point(px(-target.clamp(0., limit)), px(0.)));
        }
        self.query.clear();
        let placeholder = self.search_placeholder();
        self.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx);
            input.set_value("", window, cx);
        });
        self.rebuild(cx);
        self.focus_handle.focus(window);
    }

    pub fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_composing(window, cx) {
            return;
        }
        let key = event.keystroke.key.as_str();
        if key == "escape" {
            cx.emit(EmojiPanelEvent::Back);
            cx.stop_propagation();
            return;
        }
        if event.keystroke.modifiers.secondary() && matches!(key, "k" | "f") {
            self.focus(window, cx);
            cx.stop_propagation();
            return;
        }
        if key == "enter" {
            if !event.is_held {
                self.paste(self.selected, cx);
            }
            cx.stop_propagation();
            return;
        }
        let input_focused = self.input.focus_handle(cx).is_focused(window);
        let recent = if self.kind == 0 && self.category == 1 && self.query.is_empty() {
            self.recent_len
        } else {
            0
        };
        let selected = match key {
            "down" | "up" => vertical_selection(
                self.selected,
                key == "down",
                self.columns,
                recent,
                self.entries.len(),
            ),
            "right" if !input_focused => self.selected.saturating_add(1),
            "left" if !input_focused => self.selected.saturating_sub(1),
            _ => return,
        };
        if !self.entries.is_empty() {
            self.selected = selected.min(self.entries.len() - 1);
            let row = if self.recent_len > 0 && self.category == 1 && self.query.is_empty() {
                if self.selected < self.recent_len {
                    0
                } else {
                    2 + (self.selected - self.recent_len) / self.columns
                }
            } else {
                self.selected / self.columns
            };
            self.scroll.scroll_to_item(row, ScrollStrategy::Center);
            self.focus_handle.focus(window);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub fn is_composing(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.input.update(cx, |input, cx| {
            input.marked_text_range(window, cx).is_some()
        })
    }
}

impl Render for EmojiPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme =
            ClippiTheme::from_settings(&self.state.read(cx).settings, Some(window.appearance()));
        let columns = ((f32::from(window.viewport_size().width) - 54.)
            / if self.kind == 1 { 125. } else { 40. })
        .floor()
        .max(if self.kind == 1 { 2. } else { 4. }) as usize;
        if columns != self.columns {
            self.columns = columns;
            self.rebuild(cx);
        }
        let version = crate::core::i18n::lang_version();
        if version != self.lang_version {
            self.lang_version = version;
            let placeholder = self.search_placeholder();
            self.input.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx)
            });
        }
        let categories = self.categories();
        let text_2 = theme.text_2;
        let mut tabs = filter_toolbar().gap(px(3.)).mb(px(4.));
        for (index, label) in [
            "Emoji",
            I18nKey::EmojiKaomoji.text(),
            I18nKey::EmojiSymbols.text(),
        ]
        .into_iter()
        .enumerate()
        {
            tabs = tabs.child(
                filter_chip(("emoji-kind", index), self.kind == index, &theme)
                    .text_size(fs(11.))
                    .font_weight(if self.kind == index {
                        FontWeight::BOLD
                    } else {
                        FontWeight::default()
                    })
                    .text_color(if self.kind == index {
                        theme.accent
                    } else {
                        theme.text_2
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.kind = index;
                            this.category_scroll.set_offset(Point::default());
                            this.choose_category(1, window, cx);
                        }),
                    )
                    .child(label),
            );
        }
        let mut choices_ui = Vec::new();
        for (index, &(zh, en, icon)) in categories.iter().enumerate() {
            let label = if crate::core::i18n::is_en() { en } else { zh };
            let color = if self.category == index {
                theme.accent
            } else {
                theme.text_2
            };
            let mut button = super::components::scroll_filter_bar::scroll_filter_chip(
                ("emoji-cat", index),
                self.category == index,
                &theme,
                32.,
            )
            .text_size(fs(12.))
            .text_color(color)
            .tooltip(move |window, cx| {
                Tooltip::element(move |_, _| div().text_size(fs(10.)).child(label))
                    .build(window, cx)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| this.choose_category(index, window, cx)),
            );
            if self.kind == 0 || index == 0 {
                button = button.child(
                    svg()
                        .path(format!("picker-icons/{icon}.svg"))
                        .size(fs(12.))
                        .text_color(color),
                );
            } else {
                button = button.child(icon);
            }
            choices_ui.push(button);
        }
        let category_bar = super::components::scroll_filter_bar::scroll_filter_bar(
            "emoji-categories",
            &theme,
            &self.category_scroll,
            cx.entity().entity_id(),
            choices_ui,
            super::components::scroll_filter_bar::ScrollFilterLayout {
                chip_width: 32.,
                chip_gap: 3.,
                scroll_left: &self.category_scroll_left,
            },
        );
        let has_recent =
            self.kind == 0 && self.category == 1 && self.query.is_empty() && self.recent_len > 0;
        let title = if has_recent {
            I18nKey::EmojiRecent.text()
        } else if !self.query.is_empty() {
            I18nKey::EmojiSearchResults.text()
        } else {
            self.category_label()
        };
        let mut rows: Vec<Option<std::ops::Range<usize>>> = Vec::new();
        if has_recent {
            rows.push(Some(0..self.recent_len));
            rows.push(None);
        }
        let start = if has_recent { self.recent_len } else { 0 };
        for index in (start..self.entries.len()).step_by(self.columns) {
            rows.push(Some(index..(index + self.columns).min(self.entries.len())));
        }
        let row_count = rows.len();
        let list_theme = theme.clone();
        let content: AnyElement = if self.entries.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .px(px(16.))
                .text_size(fs(11.))
                .text_color(theme.text_2)
                .child(if self.category == 0 && self.query.is_empty() {
                    I18nKey::EmojiRecentEmpty.text()
                } else {
                    I18nKey::EmojiEmpty.text()
                })
                .into_any_element()
        } else {
            uniform_list(
                "emoji-grid",
                row_count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .map(|row_index| {
                            let mut row = div()
                                .w_full()
                                .h(px(42.))
                                .flex()
                                .px(px(8.))
                                .pr(px(18.))
                                .gap(px(2.))
                                .items_center();
                            let Some(range) = rows[row_index].clone() else {
                                return row
                                    .text_size(fs(11.))
                                    .text_color(list_theme.text_2)
                                    .child(this.category_label());
                            };
                            for index in range {
                                let entry = this.entries[index];
                                let name = entry.label().to_string();
                                let button = div()
                                    .id(("picker-entry", index))
                                    .flex_1()
                                    .min_w(px(0.))
                                    .h(px(38.))
                                    .rounded(px(6.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .overflow_hidden()
                                    .cursor_pointer()
                                    .text_color(list_theme.text_1)
                                    .text_size(fs(if this.kind == 1 { 12. } else { 24. }))
                                    .when(this.kind == 0, |button| {
                                        button.font_family(if cfg!(target_os = "windows") {
                                            "Segoe UI Emoji"
                                        } else {
                                            "Apple Color Emoji"
                                        })
                                    })
                                    .when(this.kind == 1, |button| button.bg(list_theme.surface))
                                    .border(px(1.))
                                    .border_color(if index == this.selected {
                                        list_theme.accent
                                    } else {
                                        rgba(0x00000000)
                                    })
                                    .when(index == this.selected, |button| {
                                        button.bg(list_theme.accent_soft)
                                    })
                                    .hover(|style| style.bg(list_theme.btn_hover))
                                    .tooltip(move |window, cx| {
                                        let name = name.clone();
                                        Tooltip::element(move |_, _| {
                                            div().text_size(fs(11.)).child(name.clone())
                                        })
                                        .build(window, cx)
                                    })
                                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                                        if *hovered && this.selected != index {
                                            this.selected = index;
                                            cx.notify();
                                        }
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| this.paste(index, cx)),
                                    )
                                    .child(entry.text.clone());
                                row = row.child(button);
                            }
                            let used = if let Some(range) = &rows[row_index] {
                                range.len()
                            } else {
                                0
                            };
                            for _ in used..this.columns {
                                row = row.child(div().flex_1());
                            }
                            row
                        })
                        .collect()
                }),
            )
            .track_scroll(self.scroll.clone())
            .flex_1()
            .min_h(px(0.))
            .into_any_element()
        };
        div()
            .id("emoji-panel")
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .size_full()
            .min_h(px(0.))
            .capture_key_down(cx.listener(Self::handle_key))
            .child(tabs)
            .child(category_bar)
            .child(
                div()
                    .px(px(12.))
                    .py(px(5.))
                    .text_size(fs(11.))
                    .text_color(text_2)
                    .child(title),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .child(content)
                    .when(!self.entries.is_empty(), |area| {
                        area.child(
                            div()
                                .absolute()
                                .top(px(0.))
                                .right(px(0.))
                                .bottom(px(0.))
                                .w(px(16.))
                                .child(
                                    Scrollbar::vertical(&self.scroll)
                                        .scrollbar_show(ScrollbarShow::Always),
                                ),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::vertical_selection;

    #[test]
    fn vertical_navigation_respects_partial_recent_row() {
        assert_eq!(vertical_selection(0, true, 8, 1, 25), 1);
        assert_eq!(vertical_selection(1, false, 8, 1, 25), 0);
        assert_eq!(vertical_selection(8, false, 8, 1, 25), 0);
        assert_eq!(vertical_selection(2, true, 8, 3, 25), 5);
        assert_eq!(vertical_selection(5, false, 8, 3, 25), 2);
        assert_eq!(vertical_selection(3, true, 8, 3, 25), 11);
        assert_eq!(vertical_selection(11, false, 8, 3, 25), 3);
    }

    #[test]
    fn vertical_navigation_handles_regular_and_empty_grids() {
        assert_eq!(vertical_selection(0, true, 8, 0, 25), 8);
        assert_eq!(vertical_selection(8, false, 8, 0, 25), 0);
        assert_eq!(vertical_selection(24, true, 8, 0, 25), 24);
        assert_eq!(vertical_selection(0, true, 8, 0, 0), 0);
        assert_eq!(vertical_selection(0, true, 8, 1, 1), 0);
    }
}
