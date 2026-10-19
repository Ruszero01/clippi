//! Compact window chrome with animated shortcut hints.

use crate::ui::font::fs;
use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use std::sync::{Arc, OnceLock};

use crate::core::i18n_keys::I18nKey;
use crate::state::app::AppState;

use super::clipboard_list::ClipboardListView;
use super::components::shortcut_hint::{shortcut_popover, shortcut_progress};
use super::theme::ClippiTheme;

const TITLEBAR_BUTTON_SIZE: f32 = 28.0;

/// Shared hit area and hover treatment for every titlebar action.
fn titlebar_button(id: &'static str, theme: &ClippiTheme) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(TITLEBAR_BUTTON_SIZE))
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| style.bg(theme.btn_hover))
}

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
    ToggleTransfer,
    ToggleEmoji,
    OpenSettings,
}

pub struct Titlebar {
    state: Entity<AppState>,
    list_view: Entity<ClipboardListView>,
    pinned: bool,
    emoji_active: bool,
    alt_shortcuts_available: bool,
    emoji_shortcut_available: bool,
    hotkeys_shortcut_available: bool,
    transfer_shortcut_available: bool,
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
            emoji_active: false,
            alt_shortcuts_available: false,
            emoji_shortcut_available: false,
            hotkeys_shortcut_available: false,
            transfer_shortcut_available: false,
            theme,
        }
    }

    pub fn set_pinned(&mut self, pinned: bool, cx: &mut Context<Self>) {
        self.pinned = pinned;
        cx.notify();
    }

    pub fn set_emoji_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.emoji_active != active {
            self.emoji_active = active;
            cx.notify();
        }
    }

    pub fn set_alt_shortcuts_available(
        &mut self,
        available: bool,
        emoji_available: bool,
        hotkeys_available: bool,
        transfer_available: bool,
        cx: &mut Context<Self>,
    ) {
        if self.alt_shortcuts_available != available
            || self.emoji_shortcut_available != emoji_available
            || self.hotkeys_shortcut_available != hotkeys_available
            || self.transfer_shortcut_available != transfer_available
        {
            self.alt_shortcuts_available = available;
            self.emoji_shortcut_available = emoji_available;
            self.hotkeys_shortcut_available = hotkeys_available;
            self.transfer_shortcut_available = transfer_available;
            cx.notify();
        }
    }

    pub fn set_theme(&mut self, theme: ClippiTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }
}

impl EventEmitter<TitlebarEvent> for Titlebar {}

impl Render for Titlebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let state = self.state.read(cx);
        let alt_pressed = state.alt_pressed && state.settings.alt_shortcuts_enabled;
        let emoji_enabled = state.settings.emoji_panel_enabled;
        let favorite_hint_progress = shortcut_progress(
            "titlebar-favorites-shortcut",
            alt_pressed && self.alt_shortcuts_available && show_favorites_filter,
            window,
            cx,
        );
        let emoji_hint_progress = shortcut_progress(
            "titlebar-emoji-shortcut",
            alt_pressed && self.emoji_shortcut_available && emoji_enabled,
            window,
            cx,
        );
        let (transfer_active, show_transfer_filter) = {
            let state = self.state.read(cx);
            (
                state.transfer_filter_active,
                state.settings.transfer_station_enabled,
            )
        };
        let hotkeys_hint_progress = shortcut_progress(
            "titlebar-hotkeys-shortcut",
            alt_pressed && self.hotkeys_shortcut_available && show_hotkeys_filter,
            window,
            cx,
        );
        let transfer_hint_progress = shortcut_progress(
            "titlebar-transfer-shortcut",
            alt_pressed && self.transfer_shortcut_available && show_transfer_filter,
            window,
            cx,
        );
        let pinned = self.pinned;
        let emoji_active = self.emoji_active;
        let emoji_titlebar = cx.entity().clone();
        let accent = theme.accent;
        let text_2 = theme.text_2;
        let fav_color = theme.fav_color;
        let hotkey_state = self.state.clone();
        let hotkey_list_view = self.list_view.clone();
        let fav_state = self.state.clone();
        let fav_list_view = self.list_view.clone();
        let transfer_titlebar = cx.entity().clone();
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
            .h(px(30.0))
            .flex_shrink_0()
            .rounded_t(px(crate::core::frontend::PANEL_RADIUS))
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
                            .text_size(fs(12.))
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
                    .when(emoji_enabled, |bar| {
                        bar.child(
                            div()
                                .relative()
                                .child(
                                    titlebar_button("titlebar-emoji", theme)
                                        .when(emoji_active, |button| button.bg(theme.accent_soft))
                                        .tooltip(|window, cx| {
                                            Tooltip::element(|_, _| {
                                                div()
                                                    .text_size(fs(10.))
                                                    .child(I18nKey::EmojiPanel.text())
                                            })
                                            .build(window, cx)
                                        })
                                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                            emoji_titlebar.update(cx, |_, cx| {
                                                cx.emit(TitlebarEvent::ToggleEmoji)
                                            });
                                        })
                                        .child(
                                            svg()
                                                .path("picker-icons/smile.svg")
                                                .size(px(17.))
                                                .text_color(if emoji_active {
                                                    accent
                                                } else {
                                                    text_2
                                                }),
                                        ),
                                )
                                .when(
                                    self.emoji_shortcut_available && emoji_hint_progress > 0.001,
                                    |slot| {
                                        slot.child(shortcut_popover(
                                            "E",
                                            emoji_hint_progress,
                                            theme,
                                        ))
                                    },
                                ),
                        )
                    })
                    // --- Custom hotkey filter button (28x28) ---
                    .when(show_hotkeys_filter, |bar| {
                        bar.child(
                            div()
                                .relative()
                                .child(
                                    titlebar_button("titlebar-hotkeys-filter", theme)
                                        .tooltip(|window, cx| {
                                            let label = I18nKey::TitlebarTooltipHotkeys.text();
                                            Tooltip::element(move |_window, _cx| {
                                                div().text_size(fs(10.)).child(label)
                                            })
                                            .build(window, cx)
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            move |_ev, _window, cx| {
                                                let items =
                                                    hotkey_state.update(cx, |state, _cx| {
                                                        state.toggle_hotkeys_filter();
                                                        state.visible_items()
                                                    });
                                                hotkey_list_view.update(cx, |list, cx| {
                                                    list.set_items(items, cx)
                                                });
                                                hotkey_titlebar
                                                    .update(cx, |_titlebar, cx| cx.notify());
                                            },
                                        )
                                        .child(
                                            div()
                                                .text_size(fs(15.))
                                                .font_family("iconfont")
                                                .text_color(if hotkeys_active {
                                                    accent
                                                } else {
                                                    text_2
                                                })
                                                .child("\u{e66b}"),
                                        ),
                                )
                                .when(
                                    self.hotkeys_shortcut_available
                                        && hotkeys_hint_progress > 0.001,
                                    |slot| {
                                        slot.child(shortcut_popover(
                                            "H",
                                            hotkeys_hint_progress,
                                            theme,
                                        ))
                                    },
                                ),
                        )
                    })
                    // --- Transfer station filter button (28x28) ---
                    .when(show_transfer_filter, |bar| {
                        bar.child(
                            div()
                                .relative()
                                .child(
                                    titlebar_button("titlebar-transfer-filter", theme)
                                        .tooltip(|window, cx| {
                                            let label = I18nKey::TransferStation.text();
                                            Tooltip::element(move |_window, _cx| {
                                                div().text_size(fs(10.)).child(label)
                                            })
                                            .build(window, cx)
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            move |_ev, _window, cx| {
                                                transfer_titlebar.update(cx, |_titlebar, cx| {
                                                    cx.emit(TitlebarEvent::ToggleTransfer);
                                                });
                                            },
                                        )
                                        .child(
                                            div()
                                                .text_size(fs(15.))
                                                .font_family("iconfont")
                                                .text_color(if transfer_active {
                                                    accent
                                                } else {
                                                    text_2
                                                })
                                                .child("\u{e794}"),
                                        ),
                                )
                                .when(
                                    self.transfer_shortcut_available
                                        && transfer_hint_progress > 0.001,
                                    |slot| {
                                        slot.child(shortcut_popover(
                                            "T",
                                            transfer_hint_progress,
                                            theme,
                                        ))
                                    },
                                ),
                        )
                    })
                    // --- Fav filter button (28x28) ---
                    .when(show_favorites_filter, |bar| {
                        bar.child(
                            div()
                                .relative()
                                .child(
                                    titlebar_button("titlebar-favorites-filter", theme)
                                        .tooltip(|window, cx| {
                                            let label = I18nKey::TitlebarTooltipFavorites.text();
                                            Tooltip::element(move |_window, _cx| {
                                                div().text_size(fs(10.)).child(label)
                                            })
                                            .build(window, cx)
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            move |_ev, _window, cx| {
                                                let items = fav_state.update(cx, |state, _cx| {
                                                    state.toggle_favorites_filter();
                                                    state.visible_items()
                                                });
                                                fav_list_view.update(cx, |list, cx| {
                                                    list.set_items(items, cx)
                                                });
                                                fav_titlebar
                                                    .update(cx, |_titlebar, cx| cx.notify());
                                            },
                                        )
                                        .child(
                                            div()
                                                .text_size(fs(15.))
                                                .font_family("iconfont")
                                                .text_color(if fav_active {
                                                    fav_color
                                                } else {
                                                    text_2
                                                })
                                                .child("\u{e630}"),
                                        ),
                                )
                                .when(
                                    self.alt_shortcuts_available && favorite_hint_progress > 0.001,
                                    |slot| {
                                        slot.child(shortcut_popover(
                                            "D",
                                            favorite_hint_progress,
                                            theme,
                                        ))
                                    },
                                ),
                        )
                    })
                    // --- Pin button (28x28) ---
                    .child(
                        titlebar_button("titlebar-pin", theme)
                            .tooltip(move |window, cx| {
                                let label = if pinned {
                                    I18nKey::TitlebarTooltipUnpin.text()
                                } else {
                                    I18nKey::TitlebarTooltipPin.text()
                                };
                                Tooltip::element(move |_window, _cx| {
                                    div().text_size(fs(10.)).child(label)
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
                                    .text_size(fs(15.))
                                    .font_family("iconfont")
                                    .text_color(if pinned { accent } else { text_2 })
                                    .child("\u{e633}"),
                            ),
                    )
                    // --- Settings button (28x28) ---
                    .child(
                        titlebar_button("titlebar-settings", theme)
                            .tooltip(|window, cx| {
                                Tooltip::element(|_, _| {
                                    div()
                                        .text_size(fs(10.))
                                        .child(I18nKey::SettingsTitle.text())
                                })
                                .build(window, cx)
                            })
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                settings_titlebar.update(cx, |_titlebar, cx| {
                                    cx.emit(TitlebarEvent::OpenSettings);
                                });
                            })
                            .child(
                                div()
                                    .text_size(fs(16.))
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
