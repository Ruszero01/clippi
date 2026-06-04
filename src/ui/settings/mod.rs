//! Settings panel — scrollable settings UI with tabs.
//!
//! Matches the original Slint `SettingsPanel.slint` layout:
//! - Top navigation bar: back button (← icon) + "Settings" title (36px)
//! - Tab bar: 5 equal-width tabs (General/Clipboard/Hotkey/Data/Sync)
//!   with accent-green underline for active tab
//! - Scrollable content area routed by active tab index
//!
//! Individual settings controls will be added in follow-up work.
//! Tab rendering methods (`render_*_tab`) serve as extension points.

use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::scroll::ScrollableElement;

mod clipboard;
mod general;

use crate::state::app::AppState;
use crate::ui::theme::ClippiTheme;
use crate::ui::window_manager::WindowManager;

/// Events emitted by the settings panel.
pub enum SettingsEvent {
    /// User clicked the back button — return to clipboard view.
    Back,
    /// Theme setting changed — RootView should rebuild its ClippiTheme.
    ThemeChanged(String),
}

impl EventEmitter<SettingsEvent> for SettingsPanel {}

/// The settings panel entity.
pub struct SettingsPanel {
    active_tab: usize,
    state: Entity<AppState>,
    window_manager: Entity<WindowManager>,
    theme: ClippiTheme,
}

const TAB_NAMES: &[&str] = &["General", "Clipboard", "Hotkey", "Data", "Sync"];

impl SettingsPanel {
    pub fn new(
        state: Entity<AppState>,
        window_manager: Entity<WindowManager>,
        cx: &mut Context<Self>,
    ) -> Self {
        let theme = {
            let settings = &state.read(cx).settings;
            ClippiTheme::from_setting(&settings.theme, None)
        };
        Self {
            active_tab: 0,
            state,
            window_manager,
            theme,
        }
    }

    pub fn set_tab(&mut self, tab: usize, cx: &mut Context<Self>) {
        self.active_tab = tab;
        cx.notify();
    }

    /// Reload theme from current AppState settings (called by RootView after ThemeChanged).
    pub fn reload_theme(&mut self, cx: &mut Context<Self>) {
        let new_theme = {
            let settings = &self.state.read(cx).settings;
            ClippiTheme::from_setting(&settings.theme, None)
        };
        self.theme = new_theme;
        cx.notify();
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_tab;
        let theme = &self.theme;
        let this = cx.entity().clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg)
            // ── Navigation bar (height 36px, mt 8px matching Slint y=8px) ──
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .h(px(36.))
                    .px(px(8.))
                    .mt(px(8.))
                    // Back button (28x28, iconfont ←)
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
                                    let _ = this.update(cx, |_panel, cx| {
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
            // ── Tab bar (height 36px, mt 8px matching Slint spacing) ──
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
                                let _ = this.update(cx, |panel, cx| {
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
                            .child(
                                div()
                                    .w_full()
                                    .h(px(2.))
                                    .mt(px(4.))
                                    .bg(underline_bg),
                            )
                    })),
            )
            // ── Tab content (fills remaining space, scrollable) ──
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .px(px(8.))
                            .pt(px(8.))
                            .pb(px(12.))
                            .overflow_y_scrollbar()
                            .child(match active {
                        0 => self.render_general_tab(_window, cx).into_any_element(),
                        1 => self.render_clipboard_tab(_window, cx).into_any_element(),
                        2 => self.render_hotkey_tab().into_any_element(),
                        3 => self.render_data_tab().into_any_element(),
                        4 => self.render_sync_tab().into_any_element(),
                        _ => div().into_any_element(),
                    }),
            )
        )
    }
}

// ── Reusable control helpers ──

impl SettingsPanel {
    /// Render a settings row with a toggle switch on the right.
    fn setting_row_with_toggle(
        &self,
        label: &str,
        desc: &str,
        value: bool,
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
            // Right: toggle switch (40×22px, 11px radius)
            .child(
                div()
                    .w(px(40.))
                    .h(px(22.))
                    .rounded(px(11.))
                    .bg(if value { accent } else { divider })
                    .px(px(2.))
                    .flex()
                    .items_center()
                    .when(value, |d| d.justify_end())
                    .when(!value, |d| d.justify_start())
                    .cursor(CursorStyle::PointingHand)
                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                        on_toggle(_window, cx);
                    })
                    .child(
                        // White circle knob (18×18px)
                        div()
                            .w(px(18.))
                            .h(px(18.))
                            .rounded(px(9.))
                            .bg(rgb(0xffffff)),
                    ),
            )
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
                        let btn_bg = if selected {
                            accent
                        } else {
                            rgba(0x00000000)
                        };
                        let btn_text = if selected {
                            rgb(0xffffff)
                        } else {
                            text_2
                        };
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
}

// ── Tab rendering stubs (not yet migrated) ──

impl SettingsPanel {
    fn render_hotkey_tab(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(100.))
            .text_color(self.theme.text_3)
            .text_size(px(13.))
            .child("Hotkey settings")
    }

    fn render_data_tab(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(100.))
            .text_color(self.theme.text_3)
            .text_size(px(13.))
            .child("Data settings")
    }

    fn render_sync_tab(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(100.))
            .text_color(self.theme.text_3)
            .text_size(px(13.))
            .child("Sync settings")
    }
}
