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

use crate::core::settings::AppSettings;
use crate::ui::theme::ClippiTheme;

/// Events emitted by the settings panel.
pub enum SettingsEvent {
    /// User clicked the back button — return to clipboard view.
    Back,
}

impl EventEmitter<SettingsEvent> for SettingsPanel {}

/// The settings panel entity.
pub struct SettingsPanel {
    active_tab: usize,
    settings: AppSettings,
    theme: ClippiTheme,
}

const TAB_NAMES: &[&str] = &["General", "Clipboard", "Hotkey", "Data", "Sync"];

impl SettingsPanel {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let settings = AppSettings::load();
        Self {
            active_tab: 0,
            settings,
            theme: ClippiTheme::dark(),
        }
    }

    pub fn set_tab(&mut self, tab: usize, cx: &mut Context<Self>) {
        self.active_tab = tab;
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
            // ── Tab content (fills remaining space) ──
            // TODO: add scroll support when tab content exceeds available height.
            .child(
                div()
                    .flex_1()
                    .px(px(8.))
                    .pt(px(8.))
                    .child(match active {
                        0 => self.render_general_tab().into_any_element(),
                        1 => self.render_clipboard_tab().into_any_element(),
                        2 => self.render_hotkey_tab().into_any_element(),
                        3 => self.render_data_tab().into_any_element(),
                        4 => self.render_sync_tab().into_any_element(),
                        _ => div().into_any_element(),
                    }),
            )
    }
}

// ── Tab rendering stubs ──
// Each returns a placeholder container. Replace with actual settings
// controls when migrating individual tab content from Slint.
// Signature: `fn render_*_tab(&self) -> impl IntoElement`

impl SettingsPanel {
    fn render_general_tab(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(100.))
            .text_color(self.theme.text_3)
            .text_size(px(13.))
            .child("General settings")
    }

    fn render_clipboard_tab(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(100.))
            .text_color(self.theme.text_3)
            .text_size(px(13.))
            .child("Clipboard settings")
    }

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
