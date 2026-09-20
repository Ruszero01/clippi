//! Search box — keyword input with list navigation keyboard shortcuts.
//!
//! Split out of the legacy combined `SearchBar`: this component owns only the
//! input field and its keyboard behavior. Type/tag filters live in `FilterBar`.

use crate::ui::font::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gpui::InteractiveElement;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_transitions::WindowUseTransition;

use crate::core::i18n_keys::I18nKey;
use crate::state::app::AppState;

use super::clipboard_list::{ClipboardListEvent, ClipboardListView};
use super::theme::ClippiTheme;

/// Debounce for applying search keywords: only the last stable keyword typed
/// within this window is submitted.
const SEARCH_DEBOUNCE_MS: u64 = 150;

/// Collapsed width/height of the new-entry button — matches the search field.
const NEW_ITEM_BUTTON_SIZE: f32 = 28.0;
/// How much wider the button gets when its label is revealed.
const NEW_ITEM_LABEL_SPAN: f32 = 62.0;
/// Width reserved for the label itself, inside the expanded button.
const NEW_ITEM_LABEL_WIDTH: f32 = 58.0;
/// Hover expansion duration.
const NEW_ITEM_EXPAND_MS: u64 = 240;

fn primary_modifier_pressed(modifiers: Modifiers) -> bool {
    modifiers.secondary()
}

/// Linear blend between two colors; `t` is clamped to 0..=1.
fn mix(from: Rgba, to: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    Rgba {
        r: from.r + (to.r - from.r) * t,
        g: from.g + (to.g - from.g) * t,
        b: from.b + (to.b - from.b) * t,
        a: from.a + (to.a - from.a) * t,
    }
}

/// Ease-out quint: most of the distance is covered early, then the motion
/// decays into place. The long decelerating tail is what reads as damping —
/// no overshoot, no bounce.
fn ease_out_damped(delta: f32) -> f32 {
    1.0 - (1.0 - delta).powi(5)
}

/// User actions the search box raises for the root view to resolve.
pub enum SearchBoxEvent {
    /// Compose a new clipboard entry in the editor.
    NewItem,
}

impl EventEmitter<SearchBoxEvent> for SearchBox {}

pub struct SearchBox {
    input: Entity<InputState>,
    state: Entity<AppState>,
    list_view: Entity<ClipboardListView>,
    theme: ClippiTheme,
    last_lang_version: u64,
    /// Whether the cursor is over the new-entry button, which drives its
    /// expand/collapse transition.
    new_item_hovered: bool,
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

        // Each keystroke bumps the generation; a debounced task only applies
        // its keyword if no newer keystroke superseded it.
        let generation = Arc::new(AtomicU64::new(0));
        let _subscriptions = vec![cx.subscribe(&input, move |_this, _, ev: &InputEvent, cx| {
            if !matches!(ev, InputEvent::Change) {
                return;
            }

            let my_generation = generation.fetch_add(1, Ordering::SeqCst) + 1;
            let generation = generation.clone();
            let keyword = input_for_read.read(cx).value().to_string();
            let state_for_input = state_for_input.clone();
            let list_for_input = list_for_input.clone();
            cx.spawn(async move |_this, cx| {
                // Trailing-edge debounce: wait for the input to stabilize.
                Timer::after(Duration::from_millis(SEARCH_DEBOUNCE_MS)).await;
                if generation.load(Ordering::SeqCst) != my_generation {
                    return; // superseded by a newer keystroke
                }
                let Ok(items) = state_for_input.update(cx, |state, _cx| {
                    state.set_keyword(&keyword);
                    state.visible_items()
                }) else {
                    return;
                };
                let _ = list_for_input.update(cx, |list, cx| {
                    list.set_items(items, cx);
                    // Typing a keyword is an explicit reorder intent: when
                    // favorites-first applies, follow the reorder to the top.
                    list.select_and_scroll_to_top_if_favorites_first(cx);
                });
            })
            .detach();
        })];

        Self {
            input,
            state,
            list_view,
            theme,
            last_lang_version: crate::core::i18n::lang_version(),
            new_item_hovered: false,
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
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;
        let surface = theme.surface;
        let divider = theme.divider;
        let accent = theme.accent;
        // Glyph and label are knocked out of the solid accent pill. The panel
        // background behind the button is `theme.bg`, but the surface token is a
        // step brighter in both themes, which keeps the small text legible on a
        // saturated fill.
        let knockout = theme.surface;
        let this = cx.entity();

        // --- New-entry button: hovering expands it to reveal its label, which ---
        // --- squeezes the search field next to it. The transition retargets ---
        // --- from its current value, so entering and leaving mid-flight stays ---
        // --- continuous instead of snapping.
        let expand = window
            .use_keyed_transition(
                "search-new-item-expand",
                cx,
                Duration::from_millis(NEW_ITEM_EXPAND_MS),
                |_, _| 0.0_f32,
            )
            .with_easing(ease_out_damped);
        expand.update(cx, |value, cx| {
            *value = if self.new_item_hovered { 1.0 } else { 0.0 };
            cx.notify();
        });
        let expand = *expand.evaluate(window, cx);
        let new_item_w = NEW_ITEM_BUTTON_SIZE + NEW_ITEM_LABEL_SPAN * expand;
        // Idle the button is a plain surface chip with a grey glyph. Once open
        // it is a solid accent pill — border and fill are the same color — and
        // the glyph/label are knocked out of it in the panel background color.
        let new_item_bg = mix(surface, accent, expand);
        let new_item_border = mix(divider, accent, expand);
        let new_item_fg = mix(text_2, knockout, expand);

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
                    .gap(px(6.))
                    .mb(px(6.))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .flex_1()
                            .min_w(px(0.))
                            .h(px(28.))
                            .bg(surface)
                            .rounded(px(10.))
                            .border(px(1.))
                            .border_color(divider)
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
                                                    list.select_previous(
                                                        gpui::ScrollStrategy::Top,
                                                        cx,
                                                    );
                                                });
                                                cx.stop_propagation();
                                                return;
                                            }
                                            "down" => {
                                                list.update(cx, |list, cx| {
                                                    list.select_next(
                                                        gpui::ScrollStrategy::Bottom,
                                                        cx,
                                                    );
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
                                            let plain =
                                                app_state.read(cx).settings.copy_as_plain_text;
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
                                    .text_size(fs(12.))
                                    .prefix(
                                        div()
                                            .pl(px(8.))
                                            .pr(px(3.))
                                            .text_size(fs(14.))
                                            .font_family("iconfont")
                                            .text_color(text_3)
                                            .child("\u{e688}"),
                                    ),
                            ),
                    )
                    .child({
                        let this = this.clone();
                        let hover_this = this.clone();
                        // Same shape as the search field it sits next to. The
                        // content row is laid out at its expanded width and the
                        // button clips it, so growing the pill wipes the label
                        // into view instead of reflowing the text.
                        div()
                            .id("search-new-item")
                            .flex()
                            .items_center()
                            .flex_shrink_0()
                            .w(px(new_item_w))
                            .h(px(NEW_ITEM_BUTTON_SIZE))
                            .bg(new_item_bg)
                            .rounded(px(10.))
                            .border(px(1.))
                            .border_color(new_item_border)
                            .overflow_hidden()
                            .cursor(CursorStyle::PointingHand)
                            .on_hover(move |hovered, _window, cx| {
                                hover_this.update(cx, |search, cx| {
                                    if search.new_item_hovered != *hovered {
                                        search.new_item_hovered = *hovered;
                                        cx.notify();
                                    }
                                });
                            })
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                this.update(cx, |search, cx| {
                                    // The button unmounts with the clipboard view
                                    // while it is open, so a stale hover flag
                                    // would leave it expanded on the way back.
                                    search.new_item_hovered = false;
                                    cx.emit(SearchBoxEvent::NewItem);
                                });
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    // Fixed-width content the pill clips: without
                                    // this the row would be squashed by the
                                    // shrinking container and the label would
                                    // compress instead of wiping into view.
                                    .flex_shrink_0()
                                    // Centers the glyph inside the collapsed 28px pill.
                                    .pl(px(6.))
                                    .child(
                                        div()
                                            .w(px(16.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(fs(16.))
                                            .text_color(new_item_fg)
                                            .child("+"),
                                    )
                                    .child(div().w(px(4.)))
                                    .child(
                                        div()
                                            .w(px(NEW_ITEM_LABEL_WIDTH))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_size(fs(11.))
                                            .text_color(new_item_fg)
                                            .opacity(expand)
                                            .child(I18nKey::EditNewItem.text()),
                                    ),
                            )
                    }),
            )
    }
}
