//! Settings panel 鈥?scrollable settings UI with tabs.
//!
//! Matches the original Slint `SettingsPanel.slint` layout:
//! - Top navigation bar: back button (鈫?icon) + "Settings" title (36px)
//! - Tab bar: 5 equal-width tabs (General/Clipboard/Hotkey/Data/Sync)
//!   with accent-green underline for active tab
//! - Scrollable content area routed by active tab index
//!
//! Individual settings controls will be added in follow-up work.
//! Tab rendering methods (`render_*_tab`) serve as extension points.

use std::collections::HashMap;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::InputState;
use gpui_component::scroll::{Scrollbar, ScrollbarShow};

mod clipboard;
mod data;
mod general;
pub mod hotkey;
mod sync;

use data::ResetDataDirState;
use hotkey::HotkeyConfirmAction;

use crate::state::app::AppState;
use crate::ui::add_backend::AddBackendPanel;
use crate::ui::components::toggle::{render_toggle, ToggleColors, ToggleTransitionState};
use crate::ui::theme::ClippiTheme;
use crate::ui::window_manager::WindowManager;

/// Events emitted by the settings panel.
pub enum SettingsEvent {
    /// User clicked the back button 鈥?return to clipboard view.
    Back,
    /// Theme setting changed 鈥?RootView should rebuild its ClippiTheme.
    ThemeChanged(String),
    ClipboardSettingsChanged {
        reload_items: bool,
        scroll_to_top: bool,
    },
    /// User clicked add/remove blacklist 鈥?RootView should show a ConfirmDialog.
    ShowHotkeyConfirm(HotkeyConfirmAction),
    /// Data settings error 鈥?RootView should show a toast.
    DataError(String),
}

impl EventEmitter<SettingsEvent> for SettingsPanel {}

/// The settings panel entity.
pub struct SettingsPanel {
    active_tab: usize,
    state: Entity<AppState>,
    window_manager: Entity<WindowManager>,
    theme: ClippiTheme,
    scroll_handle: ScrollHandle,
    /// Track toggle values + generation counter for transition animation.
    toggle_states: HashMap<String, ToggleTransitionState>,
    backend_collapse_states: HashMap<String, BackendCollapseState>,
    backend_panel: Entity<AddBackendPanel>,
    /// Pending hotkey blacklist confirmation (consumed by RootView).
    pub hotkey_confirm: Option<HotkeyConfirmAction>,
    /// Reset-data-directory dialog state (portable mode only).
    pub reset_data_dialog: Option<ResetDataDirState>,
    /// Whether the max-items field is in editing mode.
    editing_max_items: bool,
    /// Input entity for the max-items editor (created once in constructor).
    max_items_input: Entity<InputState>,
    /// Focus-out subscription for the max-items input (auto-save on blur).
    _max_items_focus_sub: gpui::Subscription,
}

const TAB_NAMES: &[&str] = &["General", "Clipboard", "Hotkey", "Data", "Sync"];

#[derive(Clone, Copy)]
pub(crate) struct BackendCollapseState {
    pub enabled: bool,
    pub generation: u64,
}

impl SettingsPanel {
    pub fn new(
        state: Entity<AppState>,
        window_manager: Entity<WindowManager>,
        theme: ClippiTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let max_items_input = cx.new(|cx| gpui_component::input::InputState::new(window, cx));
        let backend_panel =
            cx.new(|cx| AddBackendPanel::new(window_manager.clone(), theme.clone(), window, cx));

        // Subscribe to focus-out on the max-items InputState.
        // When the input loses focus, save and exit editing.
        let state_sub = state.clone();
        let input_sub = max_items_input.clone();
        let handle = max_items_input.read(cx).focus_handle(cx);
        let _max_items_focus_sub =
            cx.on_focus_out(&handle, window, move |this, _ev, _window, cx| {
                if this.editing_max_items {
                    let text = input_sub.read(cx).value().to_string();
                    let n: u32 = text.trim().parse().unwrap_or(0);
                    state_sub.update(cx, |s, _cx| {
                        s.settings.max_items = n;
                        s.settings.save();
                    });
                    this.editing_max_items = false;
                    cx.notify();
                }
            });

        Self {
            active_tab: 0,
            state,
            window_manager,
            theme,
            scroll_handle: ScrollHandle::default(),
            toggle_states: HashMap::new(),
            backend_collapse_states: HashMap::new(),
            backend_panel,
            hotkey_confirm: None,
            reset_data_dialog: None,
            editing_max_items: false,
            max_items_input,
            _max_items_focus_sub,
        }
    }

    /// Reload theme from the computed ClippiTheme (called by RootView after ThemeChanged).
    pub fn reload_theme(&mut self, theme: ClippiTheme, cx: &mut Context<Self>) {
        self.theme = theme.clone();
        self.backend_panel
            .update(cx, |panel, cx| panel.set_theme(theme, cx));
        cx.notify();
    }

    pub fn backend_panel(&self) -> Entity<AddBackendPanel> {
        self.backend_panel.clone()
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_tab;
        let theme = &self.theme;
        let this = cx.entity().clone();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .w_full()
            .overflow_hidden()
            .rounded_b(px(12.))
            .bg(theme.bg)
            // 鈹€鈹€ Navigation bar (height 36px, mt 8px matching Slint y=8px) 鈹€鈹€
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .h(px(36.))
                    .px(px(8.))
                    .mt(px(8.))
                    // Back button (28x28, iconfont 鈫?
                    .child(
                        div()
                            .w(px(28.))
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, {
                                let this = this.clone();
                                move |_ev, _window, cx| {
                                    this.update(cx, |_panel, cx| {
                                        cx.emit(SettingsEvent::Back);
                                    });
                                }
                            })
                            .child(
                                div()
                                    .font_family("iconfont")
                                    .text_size(px(16.))
                                    .text_color(theme.text_2)
                                    .child("\u{e62b}"),
                            ),
                    )
                    // Title "Settings" (14px, 700 weight, text_1)
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_1)
                            .child("Settings"),
                    ),
            )
            // 鈹€鈹€ Tab bar (height 36px, mt 8px matching Slint spacing) 鈹€鈹€
            .child(
                div()
                    .flex()
                    .flex_row()
                    .h(px(36.))
                    .px(px(8.))
                    .mt(px(8.))
                    .border_b(px(1.))
                    .border_color(theme.divider)
                    .children(TAB_NAMES.iter().enumerate().map(|(i, name)| {
                        let is_active = i == active;
                        let tab_color = if is_active {
                            theme.accent
                        } else {
                            theme.text_2
                        };
                        let underline_bg = if is_active {
                            theme.accent
                        } else {
                            rgba(0x00000000)
                        };
                        let this = this.clone();

                        div()
                            .flex_1()
                            .h_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                this.update(cx, |panel, cx| {
                                    panel.active_tab = i;
                                    cx.notify();
                                });
                            })
                            // Tab label
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(if is_active {
                                        FontWeight::BOLD
                                    } else {
                                        FontWeight::default()
                                    })
                                    .text_color(tab_color)
                                    .child(*name),
                            )
                            // Active underline indicator (2px)
                            .child(div().w_full().h(px(2.)).mt(px(4.)).bg(underline_bg))
                    })),
            )
            // 鈹€鈹€ Tab content (fills remaining space, scrollable) 鈹€鈹€
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .rounded_b(px(12.))
                    .bg(theme.bg)
                    .pt(px(8.))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .w_full()
                            .overflow_hidden()
                            .child(
                                div()
                                    .id("settings-scroll-area")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .track_scroll(&self.scroll_handle)
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .flex_col()
                                            .px(px(8.))
                                            .pb(px(56.))
                                            .child(match active {
                                                0 => self
                                                    .render_general_tab(window, cx)
                                                    .into_any_element(),
                                                1 => self
                                                    .render_clipboard_tab(window, cx)
                                                    .into_any_element(),
                                                2 => self
                                                    .render_hotkey_tab(window, cx)
                                                    .into_any_element(),
                                                3 => self
                                                    .render_data_tab(window, cx)
                                                    .into_any_element(),
                                                4 => self
                                                    .render_sync_tab(window, cx)
                                                    .into_any_element(),
                                                _ => div().into_any_element(),
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(4.))
                                    .right(px(0.))
                                    .bottom(px(10.))
                                    .w(px(16.))
                                    .child(
                                        Scrollbar::vertical(&self.scroll_handle)
                                            .scrollbar_show(ScrollbarShow::Scrolling),
                                    ),
                            ),
                    )
                    // 鈹€鈹€ Reset data directory dialog (overlay) 鈹€鈹€
                    .child(self.render_reset_data_dialog(window, cx).into_any_element()),
            )
    }
}

// 鈹€鈹€ Reusable control helpers 鈹€鈹€

impl SettingsPanel {
    /// Render a settings row with an animated toggle switch on the right.
    fn setting_row_with_toggle(
        &mut self,
        label: &str,
        desc: &str,
        value: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_toggle: impl Fn(&mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = &self.theme;
        let surface = theme.surface;
        let divider = theme.divider;
        let accent = theme.accent;
        let text_1 = theme.text_1;
        let text_3 = theme.text_3;

        div()
            .h(px(66.))
            .rounded(px(10.))
            .bg(surface)
            .border(px(1.))
            .border_color(divider)
            .px(px(14.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(text_3)
                            .child(desc.to_string()),
                    ),
            )
            .child(render_toggle(
                value,
                label,
                ToggleColors {
                    accent,
                    track_off: divider,
                },
                &mut self.toggle_states,
                window,
                cx,
                on_toggle,
            ))
    }

    /// Render a settings row with an option button group on the right.
    fn setting_row_with_options(
        &self,
        label: &str,
        desc: &str,
        options: &[(&'static str, &'static str)],
        active_key: &str,
        on_select: impl Fn(&'static str, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = &self.theme;
        let surface = theme.surface;
        let divider = theme.divider;
        let accent = theme.accent;
        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;
        let on_select = std::rc::Rc::new(on_select);

        div()
            .h(px(66.))
            .rounded(px(10.))
            .bg(surface)
            .border(px(1.))
            .border_color(divider)
            .px(px(14.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            // Left: label + description
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(text_3)
                            .child(desc.to_string()),
                    ),
            )
            // Right: option buttons
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(4.))
                    .children(options.iter().map(|(key, display_label)| {
                        let selected = *key == active_key;
                        let btn_bg = if selected { accent } else { rgba(0x00000000) };
                        let btn_text = if selected { rgb(0xffffff) } else { text_2 };
                        let btn_weight = if selected {
                            FontWeight::BOLD
                        } else {
                            FontWeight::default()
                        };
                        let key = *key;
                        let on_select = on_select.clone();

                        div()
                            .h(px(26.))
                            .rounded(px(7.))
                            .px(px(8.))
                            .bg(btn_bg)
                            .when(!selected, |d| d.border(px(1.)).border_color(divider))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                on_select(key, _window, cx);
                            })
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .font_weight(btn_weight)
                                    .text_color(btn_text)
                                    .child(*display_label),
                            )
                    })),
            )
    }
    /// Clear pending hotkey confirm dialog.
    pub fn clear_hotkey_confirm(&mut self, cx: &mut Context<Self>) {
        self.hotkey_confirm = None;
        cx.notify();
    }
}
