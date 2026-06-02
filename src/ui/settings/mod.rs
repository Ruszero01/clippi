//! Settings panel — scrollable settings UI with tabs.
//!
//! Contains 5 tabs: General, Clipboard, Hotkey, Data, Sync.

use gpui::*;

use crate::core::settings::AppSettings;

/// The settings panel entity.
pub struct SettingsPanel {
    active_tab: usize,
    settings: AppSettings,
}

impl SettingsPanel {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let settings = AppSettings::load();
        Self { active_tab: 0, settings }
    }

    pub fn set_tab(&mut self, tab: usize, cx: &mut Context<Self>) {
        self.active_tab = tab;
        cx.notify();
    }
}

const TAB_NAMES: &[&str] = &["General", "Clipboard", "Hotkey", "Data", "Sync"];

impl Render for SettingsPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_tab;

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x1e1f23))
            // Tab bar
            .child(
                div()
                    .flex()
                    .flex_row()
                    .bg(rgb(0x25262a))
                    .border_b(px(1.))
                    .border_color(rgb(0x3d3e42))
                    .children(TAB_NAMES.iter().enumerate().map(|(i, name)| {
                        let is_active = i == active;
                        div()
                            .px(px(12.))
                            .py(px(8.))
                            .text_size(px(12.))
                            .text_color(if is_active { rgb(0xe0e0e0) } else { rgb(0x888888) })
                            .border_b(if is_active { px(2.) } else { px(0.) })
                            .border_color(if is_active { rgb(0x3d7ef5) } else { rgb(0x00000000) })
                            .cursor(CursorStyle::PointingHand)
                            .child(*name)
                    })),
            )
            // Tab content (placeholder)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(0x666666))
                    .text_size(px(14.))
                    .child(format!("{} settings coming soon", TAB_NAMES[active])),
            )
    }
}
