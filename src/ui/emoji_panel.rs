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
use crate::core::symbols;
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
const KAOMOJI_CATEGORIES: &[(&str, &str, &str)] = &[
    ("开心", "Happy", "☺"),
    ("难过", "Sad", "☂"),
    ("生气", "Angry", "ϟ"),
    ("可爱", "Cute", "♡"),
    ("打招呼", "Greetings", "✌"),
];

fn category_scroll_edges(scroll: &ScrollHandle) -> (bool, bool) {
    let offset = f32::from(scroll.offset().x);
    let limit = f32::from(scroll.max_offset().width);
    (offset < -0.5, -offset < limit - 0.5)
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

    fn categories(&self) -> &'static [(&'static str, &'static str, &'static str)] {
        match self.kind {
            1 => KAOMOJI_CATEGORIES,
            2 => symbols::CATEGORIES,
            _ => EMOJI_CATEGORIES,
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
        if self.kind == 0 && !searching && self.category <= 1 {
            for text in &self.state.read(cx).settings.recent_emojis {
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
        if searching || self.kind != 0 || self.category != 0 {
            self.entries.extend(source.iter().filter(|entry| {
                if searching {
                    entry.matches(&self.query)
                } else if self.kind == 2 {
                    symbols::in_category(entry, self.category)
                } else {
                    !entry.variant
                        && entry.group
                            == if self.kind == 0 {
                                emoji::PICKER_GROUPS[self.category - 1]
                            } else {
                                self.category
                            }
                }
            }));
        }
        if self.kind == 2 && self.category == 0 && !searching {
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
            state.paste_picker_text(&entry.text);
            cx.notify();
        });
        cx.notify();
    }

    fn choose_category(&mut self, category: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.category = category;
        self.category_scroll.scroll_to_item(category);
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
        let step = match key {
            "down" => self.columns as isize,
            "up" => -(self.columns as isize),
            "right" if !input_focused => 1,
            "left" if !input_focused => -1,
            _ => return,
        };
        if !self.entries.is_empty() {
            self.selected = self
                .selected
                .saturating_add_signed(step)
                .min(self.entries.len() - 1);
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
                            this.choose_category(if index == 0 { 1 } else { 0 }, window, cx);
                        }),
                    )
                    .child(label),
            );
        }
        // GPUI maps a vertical wheel to horizontal movement when only X scrolls.
        // Fixed-size children keep targets legible instead of squeezing every category to fit.
        let mut catbar = div()
            .flex()
            .items_center()
            .flex_1()
            .min_w(px(0.))
            .id("emoji-categories")
            .h(px(22.))
            .gap(px(3.))
            .overflow_x_scroll()
            .overflow_y_hidden()
            .track_scroll(&self.category_scroll);
        for (index, &(zh, en, icon)) in categories.iter().enumerate() {
            let label = if crate::core::i18n::is_en() { en } else { zh };
            let color = if self.category == index {
                theme.accent
            } else {
                theme.text_2
            };
            let mut button = filter_chip(("emoji-cat", index), self.category == index, &theme)
                .flex_none()
                .flex_basis(px(32.))
                .w(px(32.))
                .text_size(fs(12.))
                .text_color(color)
                .tooltip(move |window, cx| {
                    Tooltip::element(move |_, _| div().text_size(fs(11.)).child(label))
                        .build(window, cx)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| this.choose_category(index, window, cx)),
                );
            if self.kind == 0 {
                button = button.child(
                    svg()
                        .path(format!("picker-icons/{icon}.svg"))
                        .size(fs(12.))
                        .text_color(color),
                );
            } else {
                button = button.child(icon);
            }
            catbar = catbar.child(button);
        }
        let edges = category_scroll_edges(&self.category_scroll);
        let arrow = |left: bool, cx: &mut Context<Self>| {
            filter_chip(
                if left {
                    "category-scroll-left"
                } else {
                    "category-scroll-right"
                },
                false,
                &theme,
            )
            .flex_none()
            .flex_basis(px(22.))
            .w(px(22.))
            .text_color(theme.text_2)
            .hover(|style| style.bg(theme.btn_hover))
            .tooltip(move |window, cx| {
                Tooltip::element(move |_, _| {
                    div().text_size(fs(10.)).child(if left {
                        I18nKey::EmojiScrollLeft.text()
                    } else {
                        I18nKey::EmojiScrollRight.text()
                    })
                })
                .build(window, cx)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    let scroll = &this.category_scroll;
                    let step = f32::from(scroll.bounds().size.width) * 0.75;
                    let offset = f32::from(scroll.offset().x) + if left { step } else { -step };
                    let limit = f32::from(scroll.max_offset().width);
                    scroll.set_offset(point(px(offset.clamp(-limit, 0.)), px(0.)));
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                svg()
                    .path(if left {
                        "picker-icons/chevron-left.svg"
                    } else {
                        "picker-icons/chevron-right.svg"
                    })
                    .size(fs(12.))
                    .text_color(theme.text_2),
            )
        };
        let scroll = self.category_scroll.clone();
        let view_id = cx.entity().entity_id();
        let category_bar = filter_toolbar()
            .relative()
            .pb(px(0.))
            .gap(px(3.))
            .when(edges.0, |bar| bar.child(arrow(true, cx)))
            .child(catbar)
            .when(edges.1, |bar| bar.child(arrow(false, cx)))
            // Reconcile after layout, including initial display and window resizing.
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, _, cx| {
                        if category_scroll_edges(&scroll) != edges {
                            cx.notify(view_id);
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
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
                .child(
                    if self.kind == 0 && self.category == 0 && self.query.is_empty() {
                        I18nKey::EmojiRecentEmpty.text()
                    } else {
                        I18nKey::EmojiEmpty.text()
                    },
                )
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
                                    .when(index == this.selected, |button| {
                                        button
                                            .bg(list_theme.accent_soft)
                                            .border(px(1.))
                                            .border_color(list_theme.accent)
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
        let selected = self.entries.get(self.selected).copied();
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .flex_shrink_0()
                    .px(px(12.))
                    .py(px(9.))
                    .border_t(px(1.))
                    .border_color(theme.divider)
                    .child(
                        div()
                            .text_size(fs(if self.kind == 1 { 12. } else { 25. }))
                            .max_w(px(140.))
                            .overflow_hidden()
                            .text_color(theme.text_1)
                            .child(selected.map(|entry| entry.text.clone()).unwrap_or_default()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .text_size(fs(11.))
                                    .text_color(theme.text_1)
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .child(
                                        selected
                                            .map(|entry| entry.label().to_string())
                                            .unwrap_or_default(),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(fs(9.))
                                    .text_color(theme.text_2)
                                    .child(I18nKey::EmojiHints.text()),
                            ),
                    ),
            )
    }
}
