//! GPUI edit panel for clipboard text and rich-text items.

use crate::ui::font::fs;
use base64::Engine;
use gpui::prelude::*;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState, RopeExt};
use gpui_component::scroll::ScrollableElement;
use gpui_component::text::{TextView, TextViewStyle};
use gpui_component::tooltip::Tooltip;
use percent_encoding::percent_decode_str;
use std::borrow::Cow;
use std::ops::Range;

use crate::core::find_replace;
use crate::core::i18n_keys::I18nKey;
use crate::core::types::{ClipboardItem, RichData};
use crate::state::app::AppState;

use super::rich_preview;
use super::theme::ClippiTheme;

/// The find bar is a single row of fields. The split handle's drag math needs
/// its height: the bar shortens the area the rich-text split divides.
const FIND_ROW_H: f32 = 26.0;
const FIND_BAR_GAP: f32 = 4.0;
const FIND_BAR_H: f32 = FIND_ROW_H;

const TYPE_OPTIONS: [(&str, I18nKey); 9] = [
    ("plain_text", I18nKey::EditTypeText),
    ("markdown", I18nKey::EditTypeMarkdown),
    ("html", I18nKey::EditTypeHtml),
    ("link", I18nKey::EditTypeUrl),
    ("path", I18nKey::EditTypePath),
    ("color", I18nKey::EditTypeColor),
    ("email", I18nKey::EditTypeEmail),
    ("phone", I18nKey::EditTypePhone),
    ("secret", I18nKey::EditTypeSecret),
];

pub struct EditPanel {
    state: Entity<AppState>,
    content_input: Entity<InputState>,
    selected_type: String,
    type_menu_open: bool,
    last_item_id: i64,
    /// `AppState::edit_session` of the session currently loaded into the input.
    /// Comparing this instead of the item id means re-opening the same item
    /// after a cancel still reloads the editor (the item id is unchanged).
    last_session: u64,
    /// True while the editor composes a new entry — Save inserts a row instead
    /// of updating one.
    is_new: bool,
    preview_generation: u64,
    theme: ClippiTheme,
    last_lang_version: u64,
    /// 编辑器区域占内容区的比例（0.0~1.0），默认 0.5 各占一半
    split_ratio: f32,
    /// 正在拖拽分隔手柄时的鼠标起始 Y 坐标（窗口坐标）
    split_dragging: Option<Pixels>,
    /// 拖拽开始时的 split_ratio
    split_drag_start_ratio: f32,
    /// 当从富文本类型切换到纯文本类型时，缓存原始富文本和提取的纯文本，
    /// 以便切回富文本时能将纯文本编辑同步回 HTML 标签中。
    rich_cache: Option<RichTextCache>,
    /// 查找替换栏是否展开
    find_open: bool,
    find_input: Entity<InputState>,
    replace_input: Entity<InputState>,
    /// 查询词变化后把视图带到第一处命中：定位需要 Window，订阅回调里拿不到，
    /// 交给下一次 render 处理（与编辑会话的重载同一个做法）。
    pending_find_jump: bool,
    _subscriptions: Vec<Subscription>,
}

pub enum EditPanelEvent {
    Back,
    /// Save succeeded. Carries the created item id when the session was a new
    /// entry, so the caller can reveal it in the list.
    Saved(Option<i64>),
}

impl EventEmitter<EditPanelEvent> for EditPanel {}

impl EditPanel {
    pub fn new(
        state: Entity<AppState>,
        theme: ClippiTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let content_input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder(I18nKey::EditContentPlaceholder.text())
        });
        let find_input = cx
            .new(|cx| InputState::new(window, cx).placeholder(I18nKey::EditFindPlaceholder.text()));
        let replace_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(I18nKey::EditReplacePlaceholder.text())
        });

        let find_for_query = find_input.clone();
        let replace_for_query = replace_input.clone();
        let _subscriptions = vec![
            cx.subscribe(&content_input, move |this, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.preview_generation = this.preview_generation.wrapping_add(1);
                    cx.notify();
                }
            }),
            cx.subscribe(&find_input, move |this, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    let _ = find_for_query.read(cx).value();
                    // 正文输入框只有「设置光标位置」这一条定位途径，
                    // 每敲一个字就把视图带到第一处命中，至少能看见位置。
                    this.pending_find_jump = true;
                    cx.notify();
                }
            }),
            cx.subscribe(&replace_input, move |_this, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    let _ = replace_for_query.read(cx).value();
                    cx.notify();
                }
            }),
        ];

        Self {
            state,
            content_input,
            selected_type: "plain_text".into(),
            type_menu_open: false,
            last_item_id: -1,
            last_session: 0,
            is_new: false,
            preview_generation: 0,
            theme,
            last_lang_version: crate::core::i18n::lang_version(),
            split_ratio: 0.5,
            split_dragging: None,
            split_drag_start_ratio: 0.5,
            rich_cache: None,
            find_open: false,
            find_input,
            replace_input,
            pending_find_jump: false,
            _subscriptions,
        }
    }

    pub fn set_theme(&mut self, theme: ClippiTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// 查找栏里的原文本与新文本。
    fn find_query(&self, cx: &App) -> String {
        self.find_input.read(cx).value().to_string()
    }

    fn replace_text(&self, cx: &App) -> String {
        self.replace_input.read(cx).value().to_string()
    }

    /// 当前查询在正文里的命中，以及从光标处起的下一处是第几个。
    ///
    /// 每次动作都按正文现算，不缓存：查找栏里没有导航和计数，
    /// 缓存的命中只会跟正文编辑脱节。
    fn matches_from_caret(&self, cx: &App) -> (Vec<Range<usize>>, usize) {
        let text = self.content_input.read(cx).value();
        let set = find_replace::find_matches(&text, &self.find_query(cx));
        let caret = self.content_input.read(cx).cursor();
        let index = find_replace::index_at_or_after(&set.ranges, caret);
        (set.ranges, index)
    }

    /// 展开查找替换栏，并把焦点交给原文本输入框。
    fn open_find_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = true;
        self.pending_find_jump = true;
        self.find_input
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// 收起查找栏，焦点回到正文。
    fn close_find_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find_open {
            return;
        }
        self.find_open = false;
        self.pending_find_jump = false;
        self.content_input
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// 工具栏按钮：展开或收起查找栏。
    fn toggle_find_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find_open {
            self.close_find_bar(window, cx);
        } else {
            self.open_find_bar(window, cx);
        }
    }

    /// 把正文视图带到某个字节位置。
    ///
    /// 正文输入框只公开了「设置光标位置」这一条定位途径（内部的
    /// `scroll_to` 不对外），而它会连带抢走焦点，所以先记下查找栏里的焦点、
    /// 定位完再还回去 —— 否则用户点一次「替换」就没法继续在框里回车。
    fn locate_offset(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        let keep_focus = if self.find_input.read(cx).focus_handle(cx).is_focused(window) {
            Some(self.find_input.clone())
        } else if self
            .replace_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
        {
            Some(self.replace_input.clone())
        } else {
            None
        };

        let input = self.content_input.clone();
        let text_len = input.read(cx).text().len();
        let offset = offset.min(text_len);
        input.update(cx, |input, cx| {
            let position = input.text().offset_to_position(offset);
            input.set_cursor_position(position, window, cx);
        });

        if let Some(field) = keep_focus {
            field.update(cx, |field, cx| field.focus(window, cx));
        }
    }

    /// 光标之后的下一处命中：只把视图带过去，不改内容。
    ///
    /// 没有高亮时到处都是一样的，所以不提供上一个/下一个按钮；
    /// 但在原文本框里按回车可以用它一处一处看过去（光标是唯一标记）。
    fn locate_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (ranges, index) = self.matches_from_caret(cx);
        if ranges.is_empty() {
            return;
        }
        // 光标已经停在这一处（刚打完字跳过来的）时再按一次才前进。
        let caret = self.content_input.read(cx).cursor();
        let next = if ranges.get(index).is_some_and(|range| range.start == caret) {
            (index + 1) % ranges.len()
        } else {
            index
        };
        let start = ranges[next].start;
        self.locate_offset(start, window, cx);
    }

    /// 一次替换所有命中。
    fn replace_every(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.content_input.read(cx).value().to_string();
        let set = find_replace::find_matches(&text, &self.find_query(cx));
        if set.is_empty() {
            return;
        }
        let replacement = self.replace_text(cx);
        let first = set.ranges[0].start;
        let replaced = set.ranges.len();
        let updated = find_replace::replace_all(&text, &set.ranges, &replacement);

        self.apply_content(updated, window, cx);
        self.locate_offset(first, window, cx);
        // 没有计数显示，替换了多少处只能在这里告诉用户。
        self.state.update(cx, |state, _cx| {
            state.show_toast(I18nKey::EditReplaceDone.fmt(&[&replaced.to_string()]));
        });
    }

    /// 把替换结果写回输入框。
    ///
    /// `set_value` 不触发 Change 事件（也就不刷新预览），所以这里手动推进
    /// 预览代次。它同时会把视图滚回顶部，调用方紧接着定位到命中处。
    fn apply_content(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.preview_generation = self.preview_generation.wrapping_add(1);
        self.content_input.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            cx.notify();
        });
    }

    /// 工具栏下方的查找替换栏：左边原文本，右边新文本，再右边全部替换。
    ///
    /// 一个动作就够用：正文里的命中没法加底色（正文输入框只开放了
    /// 「设置光标位置」这一条定位途径），所以不做上一个/下一个、计数与
    /// 单处替换 —— 定位靠光标，替换了多少处靠替换后的提示。
    fn find_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let surface = theme.surface;
        let divider = theme.divider;
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;
        let accent = theme.accent;
        let hover_bg = if theme.bg == rgb(0x191a1b) {
            rgba(0xffffff10)
        } else {
            rgba(0x0000000a)
        };
        let this = cx.entity();

        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .flex_shrink_0()
            .gap(px(FIND_BAR_GAP))
            .on_key_down({
                let this = this.clone();
                move |ev: &KeyDownEvent, window, cx| {
                    match ev.keystroke.key.as_str() {
                        "escape" => {
                            this.update(cx, |panel, cx| panel.close_find_bar(window, cx));
                            cx.stop_propagation();
                        }
                        "enter" => {
                            this.update(cx, |panel, cx| {
                                // 在新文本框里回车＝全部替换；在原文本框里回车
                                // 只是把视图带到下一处，不动内容。
                                let in_replace = panel
                                    .replace_input
                                    .read(cx)
                                    .focus_handle(cx)
                                    .is_focused(window);
                                if in_replace {
                                    panel.replace_every(window, cx);
                                } else {
                                    panel.locate_next(window, cx);
                                }
                            });
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }
            })
            .child(find_field(
                &self.find_input,
                surface,
                divider,
                text_3,
                "\u{e64c}",
            ))
            .child(find_field(
                &self.replace_input,
                surface,
                divider,
                text_3,
                "\u{e7a9}",
            ))
            .child(icon_button(
                "edit-replace-all",
                "\u{e6ed}",
                text_2,
                accent,
                hover_bg,
                Some(I18nKey::EditReplaceAll.text()),
                {
                    let this = this.clone();
                    move |window, cx| {
                        this.update(cx, |panel, cx| panel.replace_every(window, cx));
                    }
                },
            ))
            .into_any_element()
    }

    fn sync_from_item(
        &mut self,
        item: &ClipboardItem,
        session: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.last_item_id = item.id;
        self.last_session = session;
        let item_type = editor_type_from_item(item);
        self.selected_type = item_type.to_string();
        self.type_menu_open = false;
        self.rich_cache = None;
        self.preview_generation = self.preview_generation.wrapping_add(1);
        // 换条目就是新的一次编辑：查找栏收起、两个输入框清空，
        // 下一条不会带着上一条的查找词和替换词。
        self.find_open = false;
        self.pending_find_jump = false;
        self.find_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.replace_input
            .update(cx, |input, cx| input.set_value("", window, cx));

        // --- For HTML items, load the raw HTML from rich_data so the ---
        // --- preview can render colored <span> tags properly.         ---
        let content = if item_type == "html" {
            let rich = RichData::from_json(&item.rich_data);
            SharedString::from(rich.html.unwrap_or_else(|| item.full_text.clone()))
        } else {
            SharedString::from(item.full_text.clone())
        };
        self.content_input.update(cx, |input, cx| {
            input.set_value(content.clone(), window, cx);
            input.focus_handle(cx).focus(window);
        });
    }

    fn apply_content_transform(
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut App,
        transform: impl FnOnce(&str) -> String,
    ) {
        input.update(cx, |input, cx| {
            let current = input.value().to_string();
            input.set_value(SharedString::from(transform(&current)), window, cx);
        });
    }

    fn save(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let text = self.content_input.read(cx).value().to_string();
        let editor_type = self.selected_type.clone();
        if self.is_new {
            // Rejected saves (blank content, write failure) keep the editor
            // open; AppState surfaces the reason as a toast.
            let created = self
                .state
                .update(cx, |state, _cx| state.save_new_item(&text, &editor_type));
            if let Some(id) = created {
                self.rich_cache = None;
                cx.emit(EditPanelEvent::Saved(Some(id)));
            }
            return;
        }

        let item_id = self.last_item_id;
        if item_id < 0 {
            return;
        }
        let saved = self.state.update(cx, |state, _cx| {
            state.save_edited_item(item_id, &text, &editor_type)
        });
        if saved {
            self.rich_cache = None;
            cx.emit(EditPanelEvent::Saved(None));
        }
    }
}

impl Render for EditPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 语言切换时刷新 InputState placeholder
        let current = crate::core::i18n::lang_version();
        if self.last_lang_version != current {
            self.last_lang_version = current;
            self.content_input.update(cx, |state, cx| {
                state.set_placeholder(I18nKey::EditContentPlaceholder.text(), window, cx);
            });
            self.find_input.update(cx, |state, cx| {
                state.set_placeholder(I18nKey::EditFindPlaceholder.text(), window, cx);
            });
            self.replace_input.update(cx, |state, cx| {
                state.set_placeholder(I18nKey::EditReplacePlaceholder.text(), window, cx);
            });
        }

        // The panel is dropped from the tree while another view is active, so a
        // cancel/close is invisible here — `edit_session` (not the item id) is
        // what tells us a fresh session must be loaded into the input.
        let (item, session, is_new) = {
            let state = self.state.read(cx);
            (
                state.editing_item.clone(),
                state.edit_session,
                state.editing_is_new,
            )
        };
        self.is_new = is_new;
        if session != self.last_session {
            if let Some(item) = item {
                self.sync_from_item(&item, session, window, cx);
            }
        }
        if self.pending_find_jump {
            self.pending_find_jump = false;
            // 从光标处往下找，光标位置比文首更接近用户想找的地方。
            let (ranges, index) = self.matches_from_caret(cx);
            if let Some(range) = ranges.get(index) {
                let start = range.start;
                self.locate_offset(start, window, cx);
            }
        }

        let this = cx.entity();
        let theme = self.theme.clone();
        let surface = theme.surface;
        let bg = theme.bg;
        let divider = theme.divider;
        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let accent = theme.accent;
        let hover_bg = if bg == rgb(0x191a1b) {
            rgba(0xffffff10)
        } else {
            rgba(0x0000000a)
        };
        let is_rich_editor = is_rich_editor_type(&self.selected_type);
        let selected_label = type_label(&self.selected_type);
        let content_input = self.content_input.clone();
        let content_text = self.content_input.read(cx).value().to_string();
        let selected_type = self.selected_type.clone();
        let preview_generation = self.preview_generation;
        let find_bar = self.find_bar(cx);
        let is_find_open = self.find_open;

        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(bg)
            .rounded_b(px(12.))
            .overflow_hidden()
            .p(px(8.))
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .h(px(36.))
                    .child(icon_button(
                        "edit-back",
                        "\u{e62b}",
                        text_2,
                        accent,
                        hover_bg,
                        Some(I18nKey::EditTooltipBack.text()),
                        {
                            let this = this.clone();
                            move |_window, cx| {
                                this.update(cx, |_panel, cx| {
                                    cx.emit(EditPanelEvent::Back);
                                });
                            }
                        },
                    ))
                    .child(
                        div()
                            .text_size(fs(14.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(if self.is_new {
                                I18nKey::EditPanelTitleNew.text()
                            } else {
                                I18nKey::EditPanelTitle.text()
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(24.))
                    .gap(px(4.))
                    .child(
                        div()
                            .h(px(22.))
                            .px(px(8.))
                            .rounded(px(4.))
                            .border(px(1.))
                            .border_color(if self.type_menu_open { accent } else { divider })
                            .bg(surface)
                            .flex()
                            .items_center()
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, {
                                let this = this.clone();
                                move |_ev, _window, cx| {
                                    this.update(cx, |panel, cx| {
                                        panel.type_menu_open = !panel.type_menu_open;
                                        cx.notify();
                                    });
                                }
                            })
                            .child(
                                div()
                                    .text_size(fs(10.))
                                    .text_color(accent)
                                    .child(selected_label),
                            ),
                    )
                    .child(div().flex_1())
                    .child(icon_button(
                        "edit-find",
                        "\u{e64c}",
                        if self.find_open { accent } else { text_2 },
                        accent,
                        hover_bg,
                        Some(I18nKey::EditTooltipFindReplace.text()),
                        {
                            let this = this.clone();
                            move |window, cx| {
                                this.update(cx, |panel, cx| panel.toggle_find_bar(window, cx));
                            }
                        },
                    ))
                    .child(icon_button(
                        "edit-url-decode",
                        "\u{e6da}",
                        text_2,
                        accent,
                        hover_bg,
                        Some(I18nKey::EditTooltipUrlDecode.text()),
                        {
                            let input = content_input.clone();
                            move |window, cx| {
                                EditPanel::apply_content_transform(&input, window, cx, |text| {
                                    percent_decode_str(text)
                                        .decode_utf8()
                                        .unwrap_or(Cow::Borrowed(text))
                                        .into_owned()
                                });
                            }
                        },
                    ))
                    .child(icon_button(
                        "edit-base64-decode",
                        "\u{e66e}",
                        text_2,
                        accent,
                        hover_bg,
                        Some(I18nKey::EditTooltipBase64Decode.text()),
                        {
                            let input = content_input.clone();
                            move |window, cx| {
                                EditPanel::apply_content_transform(
                                    &input,
                                    window,
                                    cx,
                                    decode_base64,
                                );
                            }
                        },
                    ))
                    .child(icon_button(
                        "edit-json-format",
                        "\u{e819}",
                        text_2,
                        accent,
                        hover_bg,
                        Some(I18nKey::EditTooltipJsonFormat.text()),
                        {
                            let input = content_input.clone();
                            move |window, cx| {
                                EditPanel::apply_content_transform(&input, window, cx, json_format);
                            }
                        },
                    ))
                    .child(icon_button(
                        "edit-trim",
                        "\u{e6db}",
                        text_2,
                        accent,
                        hover_bg,
                        Some(I18nKey::EditTooltipTrim.text()),
                        {
                            let input = content_input.clone();
                            move |window, cx| {
                                EditPanel::apply_content_transform(&input, window, cx, trim_text);
                            }
                        },
                    )),
            )
            .when(is_find_open, move |panel| panel.child(find_bar))
            .child({
                let split_ratio = self.split_ratio;
                let is_dragging = self.split_dragging.is_some();
                // 查找栏展开后内容区变矮，拖拽比例要按同一条基准换算
                let find_bar_h = if self.find_open { FIND_BAR_H } else { 0.0 };
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .overflow_hidden()
                    .when(is_dragging, |el| {
                        // 拖拽时在整个区域监听鼠标移动和释放，防止鼠标移出手柄后丢失跟踪
                        el.on_mouse_move({
                            let this = this.clone();
                            move |ev, window, cx| {
                                this.update(cx, |panel, cx| {
                                    if let Some(start_y) = panel.split_dragging {
                                        let delta = f32::from(ev.position.y) - f32::from(start_y);
                                        // 从窗口高度估算内容区高度（减去 header/toolbar/button/gap/padding ≈ 132px）
                                        let content_h = (f32::from(window.viewport_size().height)
                                            - 132.0
                                            - find_bar_h)
                                            .max(200.0);
                                        let delta_ratio = delta / content_h;
                                        let new_ratio = (panel.split_drag_start_ratio
                                            + delta_ratio)
                                            .clamp(0.15, 0.85);
                                        panel.split_ratio = new_ratio;
                                        cx.notify();
                                    }
                                });
                            }
                        })
                        .on_mouse_up(MouseButton::Left, {
                            let this = this.clone();
                            move |_ev, _window, cx| {
                                this.update(cx, |panel, cx| {
                                    panel.split_dragging = None;
                                    cx.notify();
                                });
                            }
                        })
                    })
                    .when(!is_rich_editor, |area| {
                        area.child(editor_box(&content_input, surface, divider, px(0.), true))
                    })
                    .when(is_rich_editor, |area| {
                        area.child(
                            editor_box(&content_input, surface, divider, px(0.), false)
                                .h(relative(split_ratio)),
                        )
                        .child(
                            // 分隔拖拽手柄
                            div()
                                .h(px(4.))
                                .w_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor(CursorStyle::ResizeUpDown)
                                .on_mouse_down(MouseButton::Left, {
                                    let this = this.clone();
                                    move |ev, _window, cx| {
                                        this.update(cx, |panel, cx| {
                                            panel.split_dragging = Some(ev.position.y);
                                            panel.split_drag_start_ratio = panel.split_ratio;
                                            cx.notify();
                                        });
                                    }
                                })
                                .on_mouse_up(MouseButton::Left, {
                                    let this = this.clone();
                                    move |_ev, _window, cx| {
                                        this.update(cx, |panel, cx| {
                                            panel.split_dragging = None;
                                            cx.notify();
                                        });
                                    }
                                })
                                .child(
                                    // 手柄视觉元素 — hover 时高亮
                                    div()
                                        .w(px(32.))
                                        .h(px(3.))
                                        .rounded(px(2.))
                                        .bg(divider)
                                        .hover(|style| style.bg(accent)),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_h(px(60.))
                                .rounded(px(8.))
                                .border(px(1.))
                                .border_color(divider)
                                .bg(surface)
                                .overflow_y_scrollbar()
                                .child(div().pt(px(8.)).pb(px(8.)).pl(px(8.)).pr(px(14.)).child(
                                    render_rich_preview(
                                        &selected_type,
                                        &content_text,
                                        self.last_item_id,
                                        preview_generation,
                                        text_1,
                                        window,
                                        cx,
                                    ),
                                )),
                        )
                    })
            })
            .child(
                div()
                    .h(px(32.))
                    .flex()
                    .flex_row()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(text_button(
                        I18nKey::BtnCancel.text(),
                        text_2,
                        divider,
                        rgba(0x00000000),
                        {
                            let this = this.clone();
                            move |_window, cx| {
                                this.update(cx, |_panel, cx| {
                                    cx.emit(EditPanelEvent::Back);
                                });
                            }
                        },
                    ))
                    .child(text_button(
                        I18nKey::EditSave.text(),
                        rgb(0xffffff),
                        accent,
                        accent,
                        {
                            let this = this.clone();
                            move |window, cx| {
                                this.update(cx, |panel, cx| panel.save(window, cx));
                            }
                        },
                    )),
            )
            .when(self.type_menu_open, |panel| {
                panel
                    .child(
                        div()
                            .absolute()
                            .size_full()
                            .on_mouse_down(MouseButton::Left, {
                                let this = this.clone();
                                move |_ev, _window, cx| {
                                    this.update(cx, |panel, cx| {
                                        panel.type_menu_open = false;
                                        cx.notify();
                                    });
                                }
                            }),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(8.))
                            .top(px(78.))
                            .w(px(112.))
                            .rounded(px(6.))
                            .border(px(1.))
                            .border_color(divider)
                            .bg(surface)
                            .shadow_lg()
                            .p(px(4.))
                            .occlude()
                            .children(TYPE_OPTIONS.into_iter().map(|(key, label_key)| {
                                let this = this.clone();
                                let key = key.to_string();
                                let label = label_key.text();
                                let active = self.selected_type == key;
                                div()
                                    .h(px(26.))
                                    .rounded(px(4.))
                                    .px(px(8.))
                                    .flex()
                                    .items_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(move |style| style.bg(hover_bg))
                                    .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                        let key = key.clone();

                                        // 提前取出 input handle，避免在 this.update 内部
                                        // 调用 input.set_value 造成 GPUI re-entrancy
                                        let input_handle = this.read(cx).content_input.clone();
                                        let mut pending_value: Option<String> = None;

                                        this.update(cx, |panel, cx| {
                                            let old_type = panel.selected_type.clone();
                                            let new_type = key.clone();
                                            let current =
                                                panel.content_input.read(cx).value().to_string();

                                            let switched = type_switch(
                                                &old_type,
                                                &new_type,
                                                &current,
                                                panel.rich_cache.take(),
                                            );
                                            panel.rich_cache = switched.cache;
                                            pending_value = switched.value;

                                            panel.selected_type = new_type;
                                            panel.type_menu_open = false;
                                            panel.preview_generation =
                                                panel.preview_generation.wrapping_add(1);
                                            cx.notify();
                                        });

                                        // 在 this.update 外部应用编辑器内容，避免重入
                                        if let Some(val) = pending_value {
                                            input_handle.update(cx, |input, cx| {
                                                input.set_value(
                                                    SharedString::from(val),
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }
                                        input_handle.update(cx, |input, cx| {
                                            input.focus_handle(cx).focus(window);
                                        });
                                    })
                                    .child(
                                        div()
                                            .text_size(fs(11.))
                                            .text_color(if active { accent } else { text_1 })
                                            .child(label),
                                    )
                            })),
                    )
            })
    }
}

fn editor_box(
    input: &Entity<InputState>,
    surface: Rgba,
    divider: Rgba,
    height: Pixels,
    fill: bool,
) -> Div {
    let box_el = div()
        .rounded(px(8.))
        .border(px(1.))
        .border_color(divider)
        .bg(surface)
        // 8px is the whole inset: gpui-component's input carries its own theme
        // padding (12px/8px at Medium) and that is removed below, so nothing
        // stacks on top of this.
        .pt(px(8.))
        .pb(px(8.))
        .pl(px(8.))
        .pr(px(0.))
        .child(
            Input::new(input)
                .appearance(false)
                .bordered(false)
                .focus_bordered(false)
                .w_full()
                .h_full()
                // Drop the theme's input padding: the box above already keeps
                // the text off the border, and the default inset is far too
                // wide for an editor that fills the panel.
                .px(px(0.))
                .py(px(0.))
                .text_size(fs(12.)),
        );
    if fill {
        box_el.flex_1()
    } else {
        box_el.h(height).min_h(px(80.))
    }
}

fn render_rich_preview(
    selected_type: &str,
    text: &str,
    item_id: i64,
    generation: u64,
    fallback_color: Rgba,
    window: &mut Window,
    cx: &mut Context<EditPanel>,
) -> AnyElement {
    let preview_key = (item_id.max(0) as u64)
        .wrapping_mul(1_000_003)
        .wrapping_add(generation);
    let style = TextViewStyle::default()
        .paragraph_gap(rems(0.25))
        .heading_font_size(|level, base| if level <= 2 { base * 1.08 } else { base });
    if selected_type == "html" {
        // --- Try to render colored spans first, fall back to plain HTML ---
        // The full entry parses the raw buffered `rich.html` (including
        // `<head>` / `<style>` class rules); normalization + link stripping
        // remain the `TextView::html` fallback path.
        let normalized = rich_preview::normalize_clipboard_html_for_render(text);
        let stripped = rich_preview::strip_html_links(&normalized);
        if let Some(lines) = rich_preview::parse_styled_html_lines_full(text) {
            return div()
                .child(rich_preview::render_styled_html_lines(
                    lines,
                    fallback_color,
                ))
                .into_any_element();
        }
        TextView::html(("edit-html-preview", preview_key), stripped, window, cx)
            .style(style)
            .selectable(false)
            .into_any_element()
    } else {
        TextView::markdown(
            ("edit-markdown-preview", preview_key),
            rich_preview::strip_markdown_links(text),
            window,
            cx,
        )
        .style(style)
        .selectable(false)
        .into_any_element()
    }
}

fn icon_button(
    id: &'static str,
    icon: &'static str,
    normal: Rgba,
    hover: Rgba,
    hover_bg: Rgba,
    tooltip: Option<&'static str>,
    handler: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(22.))
        .h(px(22.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::PointingHand)
        .hover(move |style| style.bg(hover_bg))
        .when_some(tooltip, |button, tip| {
            button.tooltip(move |window, cx| {
                Tooltip::element(move |_window, _cx| div().text_size(fs(10.)).child(tip))
                    .build(window, cx)
            })
        })
        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            handler(window, cx)
        })
        .child(
            div()
                .font_family("iconfont")
                .text_size(fs(14.))
                .text_color(normal)
                .hover(move |style| style.text_color(hover))
                .child(icon),
        )
}

/// 查找栏里的输入框：和编辑器的输入框同一套写法，外面套一层带边框的壳。
fn find_field(
    input: &Entity<InputState>,
    surface: Rgba,
    divider: Rgba,
    icon_color: Rgba,
    icon: &'static str,
) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .flex_1()
        .min_w(px(0.))
        .h(px(FIND_ROW_H))
        .bg(surface)
        .rounded(px(6.))
        .border(px(1.))
        .border_color(divider)
        .overflow_hidden()
        .child(
            div()
                .pl(px(6.))
                .pr(px(3.))
                .flex_shrink_0()
                .font_family("iconfont")
                .text_size(fs(12.))
                .text_color(icon_color)
                .child(icon),
        )
        .child(
            Input::new(input)
                .appearance(false)
                .bordered(false)
                .focus_bordered(false)
                .w_full()
                .h_full()
                // Same as the body: no theme padding inside the bar's own
                // 26px-high field, or the text would have no room left.
                .px(px(0.))
                .py(px(0.))
                .text_size(fs(11.)),
        )
}

fn text_button(
    label: &'static str,
    text_color: Rgba,
    border_color: Rgba,
    bg: Rgba,
    handler: impl Fn(&mut Window, &mut App) + 'static,
) -> Div {
    div()
        .w(px(60.))
        .h(px(28.))
        .rounded(px(6.))
        .border(px(1.))
        .border_color(border_color)
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::PointingHand)
        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            handler(window, cx)
        })
        .child(
            div()
                .text_size(fs(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_color)
                .child(label),
        )
}

/// The type the editor opens an entry as. Shared with the external editor's
/// read-back path, which writes content back under the same type.
fn editor_type_from_item(item: &ClipboardItem) -> &'static str {
    item.editor_type()
}

fn type_label(key: &str) -> &'static str {
    TYPE_OPTIONS
        .iter()
        .find_map(|(option_key, label_key)| (*option_key == key).then_some(label_key.text()))
        .unwrap_or(I18nKey::EditTypeText.text())
}

fn json_format(text: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| text.to_string()),
        Err(_) => text.to_string(),
    }
}

fn trim_text(text: &str) -> String {
    let text = text
        .replace("\r\n", "\n")
        .replace(['\r', '\u{2028}', '\u{2029}'], "\n");
    let mut result = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut prev_ws = false;
        for ch in trimmed.chars() {
            if ch.is_whitespace() {
                if !prev_ws {
                    result.push(' ');
                    prev_ws = true;
                }
            } else {
                result.push(ch);
                prev_ws = false;
            }
        }
        result.push('\n');
    }
    if result.ends_with('\n') {
        result.pop();
    }
    result
}

fn decode_base64(text: &str) -> String {
    let encoded = if let Some(pos) = text.find(";base64,") {
        &text[pos + 8..]
    } else {
        text
    };
    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) {
        return String::from_utf8_lossy(&bytes).into_owned();
    }
    match base64::engine::general_purpose::URL_SAFE.decode(encoded) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => text.to_string(),
    }
}

/// 是否为富文本编辑器类型（带有样式标签，如 HTML、Markdown）
fn is_rich_editor_type(t: &str) -> bool {
    matches!(t, "markdown" | "html")
}

/// HTML 是唯一一种其他类型无法承载的内容：它的缓冲区里是带标签的文档，
/// 其他类型只能编辑其中的可见文本。Markdown 与纯文本同为文本，切换时
/// 内容原样保留。
fn is_html_editor_type(t: &str) -> bool {
    t == "html"
}

/// 缓存富文本内容，支持纯文本编辑后同步回 HTML。
#[derive(Clone)]
struct RichTextCache {
    /// 原始 HTML/富文本内容（含完整标签）
    html: String,
    /// 从 HTML 中提取的纯文本（规范化后，无多余空行）
    plain: String,
    /// 提取时记录的各文本段（顺序与 HTML 中 `>text<` 一致）
    segments: Vec<String>,
}

/// 切换编辑器类型后缓冲区与缓存的新状态。
struct TypeSwitch {
    /// 需要写回输入框的内容；`None` 表示保持用户输入不变。
    value: Option<String>,
    /// 切换后的富文本缓存；`None` 表示没有可还原的 HTML。
    cache: Option<RichTextCache>,
}

/// 计算类型切换对内容的影响。
///
/// 只有 HTML 参与转换：离开 HTML 时提取可见文本、并把文档缓存起来，回到
/// HTML 时还原（期间的纯文本编辑会尝试回填到文档里）。其余任何切换——
/// 包括 Markdown 与纯文本之间——都不动缓冲区，用户输入什么条目就存什么，
/// 避免 "# 标题" 这类内容在切换类型时被悄悄改写。
fn type_switch(
    old_type: &str,
    new_type: &str,
    current: &str,
    cache: Option<RichTextCache>,
) -> TypeSwitch {
    let leaving_html = is_html_editor_type(old_type) && !is_html_editor_type(new_type);
    if leaving_html {
        let (plain, segments) = extract_text_and_segments(current);
        return TypeSwitch {
            value: Some(plain.clone()),
            cache: Some(RichTextCache {
                html: current.to_string(),
                plain,
                segments,
            }),
        };
    }

    let entering_html = !is_html_editor_type(old_type) && is_html_editor_type(new_type);
    if entering_html {
        let Some(cache) = cache else {
            // 没有可还原的 HTML（例如 Markdown 条目）：当前文本直接作为
            // HTML 源码，内容保持不变。
            return TypeSwitch {
                value: None,
                cache: None,
            };
        };
        let restored = if current == cache.plain {
            cache.html.clone()
        } else {
            replace_text_in_html(&cache.html, current, &cache.segments)
        };
        return TypeSwitch {
            value: Some(restored),
            cache: None,
        };
    }

    TypeSwitch { value: None, cache }
}

/// 从 HTML 提取纯文本的同时记录各文本段（用于反向同步）。
///
/// 只跟踪标签名（第一个空格或 `/` 之前的部分），跳过标签属性值，
/// 避免对 base64 等大数据调用 to_lowercase() 导致 UI 卡死。
fn extract_text_and_segments(html: &str) -> (String, Vec<String>) {
    let mut text = String::with_capacity(html.len());
    let mut segments: Vec<String> = Vec::new();
    let mut current_text = String::new(); // 标签外的文本
    let mut current_tag = String::new(); // 仅标签名（最多几十字节）
    let mut in_tag = false;
    let mut in_tag_name = true; // 仍在收集标签名（遇到空格或自闭合 / 后停止）
    let mut last_was_newline = false;

    for ch in html.chars() {
        if ch == '<' {
            // 结束当前文本段
            if !current_text.is_empty() {
                if !current_text.chars().all(|c| c.is_whitespace()) {
                    text.push_str(&current_text);
                    segments.push(current_text.clone());
                    last_was_newline = false;
                }
                current_text.clear();
            }
            in_tag = true;
            in_tag_name = true;
            current_tag.clear();
        } else if ch == '>' {
            in_tag = false;
            in_tag_name = false;
            let tag_lower = current_tag.to_lowercase(); // 标签名很短，安全
            if is_block_tag(&tag_lower) && !last_was_newline {
                text.push('\n');
                last_was_newline = true;
            }
            current_tag.clear();
        } else if in_tag {
            if in_tag_name {
                if current_tag.is_empty() && ch == '/' {
                    // 闭合标签：</p> → 保留 / 前缀用于 is_block_tag 匹配
                    current_tag.push(ch);
                } else if ch == '/' || ch.is_whitespace() {
                    // 自闭合 <br/> <br /> 或标签名结束 → 停止收集
                    in_tag_name = false;
                } else {
                    current_tag.push(ch);
                }
            }
            // 跳过标签属性值（不累积，避免大内存分配）
        } else {
            // 文本内容
            if last_was_newline && ch.is_whitespace() && ch != '\n' {
                // 跳过块级标签后的前导空白
                continue;
            }
            if ch == '\n' {
                if !last_was_newline {
                    current_text.push('\n');
                }
            } else {
                current_text.push(ch);
                last_was_newline = false;
            }
        }
    }

    // 收尾：最后一个文本段
    if !current_text.is_empty() {
        let trimmed: String = current_text
            .chars()
            .filter(|&c| c != '\n' || !last_was_newline)
            .collect();
        if !trimmed.chars().all(|c| c.is_whitespace()) {
            text.push_str(&trimmed);
            segments.push(trimmed);
        }
    }

    // 规范化：合并连续空行，去除首尾空行
    let mut result = String::with_capacity(text.len());
    let mut prev_newline = false;
    for ch in text.chars() {
        if ch == '\n' {
            if !prev_newline {
                result.push('\n');
                prev_newline = true;
            }
        } else {
            result.push(ch);
            prev_newline = false;
        }
    }
    let result = result.trim().to_string();

    // 解码常见 HTML 实体
    let result = result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");

    (result, segments)
}

/// 是否为块级 HTML 标签（闭合后应换行）
fn is_block_tag(tag: &str) -> bool {
    // 匹配闭合标签如 /p, /div 或自闭合/空标签如 br, hr
    tag == "/p"
        || tag == "/div"
        || tag == "/li"
        || tag == "/tr"
        || tag == "/h1"
        || tag == "/h2"
        || tag == "/h3"
        || tag == "/h4"
        || tag == "/h5"
        || tag == "/h6"
        || tag == "/table"
        || tag == "/ul"
        || tag == "/ol"
        || tag == "/blockquote"
        || tag == "/section"
        || tag == "/article"
        || tag == "/header"
        || tag == "/footer"
        || tag == "/nav"
        || tag == "/main"
        || tag == "/pre"
        || tag == "/figure"
        || tag == "/figcaption"
        || tag == "/dl"
        || tag == "/dt"
        || tag == "/dd"
        || tag == "/td"
        || tag == "/th"
        || tag == "/hr"
        || tag.starts_with("br")
}

/// 将编辑后的纯文本同步回 HTML，替换文本节点。
///
/// 策略：
/// 1. 从原 HTML 中重新提取文本段（`>text<` 模式）
/// 2. 如果新旧文本段数量一致 → 逐个替换
/// 3. 否则 → 返回原始 HTML（无法可靠映射）
fn replace_text_in_html(html: &str, new_plain: &str, old_segments: &[String]) -> String {
    // 解析新纯文本的段落（按换行拆分）
    let new_segments: Vec<&str> = new_plain.lines().collect();
    if new_segments.len() != old_segments.len() {
        // 行数不匹配，无法可靠替换，返回原始 HTML
        return html.to_string();
    }

    let mut result = html.to_string();
    for (old, new) in old_segments.iter().zip(new_segments.iter()) {
        if old == new {
            continue;
        }
        // 查找并替换第一个出现在 `>...<` 之间的匹配项
        let pattern = format!(">{}<", old);
        if let Some(pos) = result.find(&pattern) {
            let replacement = format!(">{}<", new);
            result.replace_range(pos..pos + pattern.len(), &replacement);
        }
    }
    result
}

#[cfg(test)]
mod type_switch_tests {
    use super::{type_switch, RichTextCache};

    fn cache(html: &str, plain: &str, segments: &[&str]) -> RichTextCache {
        RichTextCache {
            html: html.to_string(),
            plain: plain.to_string(),
            segments: segments.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// The reported bug: text typed as plain text was rewritten on the way to
    /// markdown. Markdown is text, so nothing about the buffer may change.
    #[test]
    fn markdown_and_plain_text_keep_the_buffer_verbatim() {
        let md = type_switch("plain_text", "markdown", "# 标题", None);
        assert_eq!(md.value, None, "plain -> markdown must not rewrite");
        assert!(md.cache.is_none());

        let plain = type_switch("markdown", "plain_text", "# 标题", None);
        assert_eq!(plain.value, None, "markdown -> plain must not rewrite");
        assert!(plain.cache.is_none());

        // Every non-HTML pair behaves the same way.
        for (from, to) in [
            ("plain_text", "link"),
            ("link", "secret"),
            ("color", "markdown"),
            ("email", "plain_text"),
        ] {
            let switched = type_switch(from, to, "keep me", None);
            assert_eq!(switched.value, None, "{from} -> {to} must not rewrite");
        }
    }

    /// Leaving HTML edits only the visible text, and the document is kept so the
    /// markup can be restored.
    #[test]
    fn leaving_html_extracts_the_visible_text_and_caches_the_document() {
        let html = "<p>hello</p><p>world</p>";
        let switched = type_switch("html", "plain_text", html, None);

        let value = switched.value.expect("buffer becomes the visible text");
        assert!(value.contains("hello") && value.contains("world"));
        assert!(!value.contains("<p>"));
        let cache = switched.cache.expect("document is cached");
        assert_eq!(cache.html, html);
        assert_eq!(cache.plain, value);

        // Leaving HTML for markdown edits the plain text too.
        let to_markdown = type_switch("html", "markdown", html, None);
        assert_eq!(to_markdown.value.as_deref(), Some(value.as_str()));
    }

    #[test]
    fn returning_to_html_restores_the_cached_document() {
        let html = "<p>hello</p>";
        let cached = cache(html, "hello", &["hello"]);

        let restored = type_switch("plain_text", "html", "hello", Some(cached.clone()));
        assert_eq!(restored.value.as_deref(), Some(html));
        assert!(restored.cache.is_none(), "the cache is consumed");

        // Edits made while away are replayed into the document.
        let edited = type_switch("markdown", "html", "goodbye", Some(cached));
        let value = edited.value.expect("buffer becomes the document again");
        assert!(
            value.contains("goodbye"),
            "edited text reaches the HTML: {value}"
        );
        assert!(!value.contains("hello"));
    }

    /// Without a cached document the buffer is already the HTML source.
    #[test]
    fn entering_html_without_a_cache_keeps_the_buffer() {
        let switched = type_switch("markdown", "html", "<p>as typed</p>", None);

        assert_eq!(switched.value, None);
        assert!(switched.cache.is_none());
    }

    /// A cache survives switches that do not touch HTML, so a detour through
    /// another type still restores the original document.
    #[test]
    fn a_cache_survives_unrelated_switches() {
        let html = "<p>hello</p>";
        let cached = cache(html, "hello", &["hello"]);

        let detour = type_switch("markdown", "plain_text", "hello", Some(cached));
        assert!(detour.value.is_none());
        let cache = detour.cache.expect("cache is kept");

        let back = type_switch("plain_text", "html", "hello", Some(cache));
        assert_eq!(back.value.as_deref(), Some(html));
    }
}
