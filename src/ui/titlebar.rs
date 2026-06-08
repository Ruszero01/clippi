//! Custom titlebar 鈥?matches original Slint design (app.slint).
//!
//! 38px height, logo + "Clippi" app name on the left (12px, 700 weight),
//! three icon buttons on the right (fav filter, pin, settings).
//! The drag area covers the left portion (width - 92px).

use gpui::*;

use crate::state::app::AppState;

use super::clipboard_list::ClipboardListView;
use super::theme::ClippiTheme;

/// Titlebar height matching original Slint design.
pub const TITLEBAR_HEIGHT: f32 = 38.0;

pub enum TitlebarEvent {
    TogglePin,
    OpenSettings,
}

pub struct Titlebar {
    state: Entity<AppState>,
    list_view: Entity<ClipboardListView>,
    pinned: bool,
    theme: ClippiTheme,
}

impl Titlebar {
    pub fn new(
        state: Entity<AppState>,
        list_view: Entity<ClipboardListView>,
        theme: ClippiTheme,
    ) -> Self {
        Self {
            state,
            list_view,
            pinned: false,
            theme,
        }
    }

    pub fn set_pinned(&mut self, pinned: bool, cx: &mut Context<Self>) {
        self.pinned = pinned;
        cx.notify();
    }

    pub fn set_theme(&mut self, theme: ClippiTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }
}

impl EventEmitter<TitlebarEvent> for Titlebar {}

impl Render for Titlebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        let fav_active = self.state.read(cx).filters.is_favorites_active();
        let pinned = self.pinned;
        let accent = theme.accent;
        let text_2 = theme.text_2;
        let fav_color = theme.fav_color;
        let state = self.state.clone();
        let list_view = self.list_view.clone();
        let fav_titlebar = cx.entity().clone();
        let pin_titlebar = cx.entity().clone();
        let settings_titlebar = cx.entity().clone();

        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(TITLEBAR_HEIGHT))
            .flex_shrink_0()
            .rounded_t(px(12.))
            .bg(theme.titlebar_bg)
            // Left: logo + app name (also serves as drag area)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .pl(px(12.))
                    .pr(px(4.))
                    .gap(px(7.))
                    .window_control_area(WindowControlArea::Drag)
                    // Logo (20x20, loaded from assets)
                    .child(
                        gpui::img(std::path::Path::new("assets/LOGO_notext.ico"))
                            .w(px(20.))
                            .h(px(20.)),
                    )
                    // App name
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_2)
                            .child("Clippi"),
                    ),
            )
            // Spacer
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            // Right: icon buttons
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.))
                    .pr(px(4.))
                    // Fav filter button (28x28)
                    .child(
                        div()
                            .w(px(28.))
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                let items = state.update(cx, |state, _cx| {
                                    state.toggle_favorites_filter();
                                    state.items.clone()
                                });
                                list_view.update(cx, |list, cx| list.set_items(items, cx));
                                fav_titlebar.update(cx, |_titlebar, cx| cx.notify());
                            })
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_family("iconfont")
                                    .text_color(if fav_active { fav_color } else { text_2 })
                                    .child("\u{e630}"),
                            ),
                    )
                    // Pin button (28x28)
                    .child(
                        div()
                            .w(px(28.))
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                pin_titlebar.update(cx, |_titlebar, cx| {
                                    cx.emit(TitlebarEvent::TogglePin);
                                });
                            })
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_family("iconfont")
                                    .text_color(if pinned { accent } else { text_2 })
                                    .child("\u{e633}"),
                            ),
                    )
                    // Settings button (28x28)
                    .child(
                        div()
                            .w(px(28.))
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                settings_titlebar.update(cx, |_titlebar, cx| {
                                    cx.emit(TitlebarEvent::OpenSettings);
                                });
                            })
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_family("iconfont")
                                    .text_color(text_2)
                                    .child("\u{e6b6}"),
                            ),
                    ),
            )
    }
}
