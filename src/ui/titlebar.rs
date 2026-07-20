//! Custom titlebar — matches original Slint design (app.slint).
//!
//! --- 38px height, logo + "Clippi" app name on the left (12px, 700 weight), ---
//! --- three icon buttons on the right (fav filter, pin, settings). ---
//! --- The drag area covers the left portion (width - 92px). ---

use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use std::sync::{Arc, OnceLock};

use crate::core::i18n_keys::I18nKey;
use crate::state::app::AppState;

use super::clipboard_list::ClipboardListView;
use super::theme::ClippiTheme;

/// Titlebar height matching original Slint design.
pub const TITLEBAR_HEIGHT: f32 = 30.0;

fn titlebar_logo_image() -> Arc<Image> {
    static LOGO: OnceLock<Arc<Image>> = OnceLock::new();
    LOGO.get_or_init(|| {
        Arc::new(Image::from_bytes(
            ImageFormat::Png,
            include_bytes!("../../assets/LOGO_notext.png").to_vec(),
        ))
    })
    .clone()
}

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
        let (hotkeys_active, fav_active, show_hotkeys_filter, show_favorites_filter) = {
            let state = self.state.read(cx);
            let hotkeys_active = state.filters.is_hotkeys_active();
            let fav_active = state.filters.is_favorites_active();
            (
                hotkeys_active,
                fav_active,
                hotkeys_active || state.has_hotkey_items,
                fav_active || state.has_favorite_items,
            )
        };
        let pinned = self.pinned;
        let accent = theme.accent;
        let text_2 = theme.text_2;
        let fav_color = theme.fav_color;
        let hotkey_state = self.state.clone();
        let hotkey_list_view = self.list_view.clone();
        let fav_state = self.state.clone();
        let fav_list_view = self.list_view.clone();
        let hotkey_titlebar = cx.entity().clone();
        let fav_titlebar = cx.entity().clone();
        let pin_titlebar = cx.entity().clone();
        let settings_titlebar = cx.entity().clone();
        let logo = titlebar_logo_image();

        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(TITLEBAR_HEIGHT))
            .flex_shrink_0()
            .rounded_t(px(12.))
            .bg(theme.titlebar_bg)
            // --- Left: logo + app name (also serves as drag area) ---
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .pl(px(12.))
                    .pr(px(4.))
                    .gap(px(7.))
                    .window_control_area(WindowControlArea::Drag)
                    // --- Logo (20x20, loaded from assets) ---
                    .child(gpui::img(logo).w(px(20.)).h(px(20.)))
                    // --- App name ---
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_2)
                            .child(I18nKey::TitlebarAppName.text()),
                    ),
            )
            // --- Spacer ---
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            // --- Right: icon buttons ---
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.))
                    .pr(px(4.))
                    // --- Custom hotkey filter button (28x28) ---
                    .when(show_hotkeys_filter, |bar| {
                        bar.child(
                            div()
                                .id("titlebar-hotkeys-filter")
                                .w(px(28.))
                                .h(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor(CursorStyle::PointingHand)
                                .tooltip(|window, cx| {
                                    let label = I18nKey::TitlebarTooltipHotkeys.text();
                                    Tooltip::element(move |_window, _cx| {
                                        div().text_size(px(10.)).child(label)
                                    })
                                    .build(window, cx)
                                })
                                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                    let items = hotkey_state.update(cx, |state, _cx| {
                                        state.toggle_hotkeys_filter();
                                        state.items.clone()
                                    });
                                    hotkey_list_view
                                        .update(cx, |list, cx| list.set_items(items, cx));
                                    hotkey_titlebar.update(cx, |_titlebar, cx| cx.notify());
                                })
                                .child(
                                    div()
                                        .text_size(px(15.))
                                        .font_family("iconfont")
                                        .text_color(if hotkeys_active { accent } else { text_2 })
                                        .child("\u{e66b}"),
                                ),
                        )
                    })
                    // --- Fav filter button (28x28) ---
                    .when(show_favorites_filter, |bar| {
                        bar.child(
                            div()
                                .id("titlebar-favorites-filter")
                                .w(px(28.))
                                .h(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor(CursorStyle::PointingHand)
                                .tooltip(|window, cx| {
                                    let label = I18nKey::TitlebarTooltipFavorites.text();
                                    Tooltip::element(move |_window, _cx| {
                                        div().text_size(px(10.)).child(label)
                                    })
                                    .build(window, cx)
                                })
                                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                    let items = fav_state.update(cx, |state, _cx| {
                                        state.toggle_favorites_filter();
                                        state.items.clone()
                                    });
                                    fav_list_view.update(cx, |list, cx| list.set_items(items, cx));
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
                    })
                    // --- Pin button (28x28) ---
                    .child(
                        div()
                            .id("titlebar-pin")
                            .w(px(28.))
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .tooltip(move |window, cx| {
                                let label = if pinned {
                                    I18nKey::TitlebarTooltipUnpin.text()
                                } else {
                                    I18nKey::TitlebarTooltipPin.text()
                                };
                                Tooltip::element(move |_window, _cx| {
                                    div().text_size(px(10.)).child(label)
                                })
                                .build(window, cx)
                            })
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
                    // --- Settings button (28x28) ---
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

#[cfg(test)]
mod tests {
    use super::titlebar_logo_image;

    #[test]
    fn titlebar_logo_is_embedded_and_decodable() {
        let logo = titlebar_logo_image();
        assert_eq!(logo.format, gpui::ImageFormat::Png);
        assert!(image::load_from_memory(&logo.bytes).is_ok());
    }
}
