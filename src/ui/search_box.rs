//! Search box — keyword input with list navigation keyboard shortcuts.
//!
//! Split out of the legacy combined `SearchBar`: this component owns only the
//! input field and its keyboard behavior. Type/tag filters live in `FilterBar`.

use gpui::InteractiveElement;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};

use crate::core::i18n_keys::I18nKey;
use crate::state::app::AppState;

use super::clipboard_list::{ClipboardListEvent, ClipboardListView};
use super::theme::ClippiTheme;

fn primary_modifier_pressed(modifiers: Modifiers) -> bool {
    modifiers.secondary()
}

pub struct SearchBox {
    input: Entity<InputState>,
    state: Entity<AppState>,
    list_view: Entity<ClipboardListView>,
    theme: ClippiTheme,
    last_lang_version: u64,
    _subscriptions: Vec<Subscription>,
}

impl SearchBox {
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
                state.visible_items()
            });
            list_for_input.update(cx, |list, cx| list.set_items(items, cx));
            cx.notify();
        })];

        Self {
            input,
            state,
            list_view,
            theme,
            last_lang_version: crate::core::i18n::lang_version(),
            _subscriptions,
        }
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
}

impl Render for SearchBox {
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
        let text_3 = theme.text_3;
        let surface = theme.surface;
        let divider = theme.divider;

        div()
            .flex()
            .flex_col()
            .w_full()
            .flex_shrink_0()
            .pt(px(1.))
            .px(px(8.))
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
                                // Ctrl/Cmd+Enter — bitmap paste for a single selected
                                // image, default paste otherwise. Floating-panel guard
                                // lives inside the list's unified action method.
                                (true, false, "enter") => {
                                    list.update(cx, |list, cx| {
                                        list.focus(window);
                                        list.action_paste_bitmap_or_default(cx);
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
                                // Ctrl+T — tag picker
                                (true, false, "t") => {
                                    list.update(cx, |list, cx| {
                                        list.focus(window);
                                        list.action_show_tag_picker(cx);
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
    }
}
