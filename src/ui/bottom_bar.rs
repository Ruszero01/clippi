//! Bottom status bar — displays the foreground application name, window title,
//! and provides quick actions (clipboard blacklist, hotkey blacklist).
//!
//! --- Matches the original Slint design: ---
//! --- - 26px height, 1px top border, divider color ---
//! --- - Left: app icon (15×15) + app name (12px) + window title (11px) ---
//! --- - Right: clipboard-blacklist + hotkey-blacklist icon buttons (22×22) ---

use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;

use crate::core::i18n_keys::I18nKey;
use crate::state::app::AppState;
use crate::ui::settings::hotkey;
use crate::ui::settings::SettingsEvent;
use crate::ui::settings::SettingsPanel;

use super::theme::ClippiTheme;

#[derive(IntoElement)]
pub struct BottomBar {
    state: Entity<AppState>,
    settings: Entity<SettingsPanel>,
    theme: ClippiTheme,
}

impl BottomBar {
    pub fn new(
        state: Entity<AppState>,
        settings: Entity<SettingsPanel>,
        theme: ClippiTheme,
    ) -> Self {
        Self {
            state,
            settings,
            theme,
        }
    }
}

impl RenderOnce for BottomBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme.clone();
        let app_state = self.state.clone();
        let settings = self.settings.clone();

        let (fg_name, fg_title) = {
            let state = app_state.read(cx);
            (
                state.foreground_app_name.clone(),
                state.foreground_window_title.clone(),
            )
        };
        let has_fg = !fg_name.is_empty();
        let foreground_icon = has_fg
            .then(|| crate::core::paths::app_icon_path(&fg_name))
            .filter(|path| path.exists());

        div()
            .h(px(26.))
            .w_full()
            .px(px(10.))
            .flex()
            .items_center()
            .justify_between()
            .border_t(px(1.))
            .border_color(theme.divider)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .min_w(px(0.))
                    .overflow_hidden()
                    .flex_1()
                    .when_some(foreground_icon, |row, image| {
                        row.child(
                            gpui::img(image)
                                .w(px(15.))
                                .h(px(15.))
                                .rounded(px(2.))
                                .flex_shrink_0(),
                        )
                    })
                    .when(has_fg, |row| {
                        row.child(
                            div()
                                .flex()
                                .items_center()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .flex_1()
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.text_2)
                                        .child(fg_name.clone()),
                                )
                                .when(!fg_title.is_empty(), |text| {
                                    text.child(
                                        div()
                                            .min_w(px(0.))
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .flex_1()
                                            .text_size(px(11.))
                                            .text_color(theme.text_3)
                                            .child(format!(
                                                " \u{2014} {}",
                                                fg_title
                                            )),
                                    )
                                }),
                        )
                    })
                    .when(!has_fg, |row| {
                        row.child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.text_3)
                                .child(I18nKey::BottomBarNoApp.text()),
                        )
                    }),
            )
            .when(has_fg, |bar| {
                bar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .child({
                            let fg = fg_name.clone();
                            let app_state = app_state.clone();
                            let settings = settings.clone();
                            let is_blacklisted = crate::core::settings::is_app_in_list(
                                &app_state.read(cx).settings.clipboard_app_blacklist,
                                &fg,
                            );

                            let base_color = if is_blacklisted { theme.fav_color } else { theme.text_3 };
                            let tooltip_label = if is_blacklisted {
                                I18nKey::ConfirmRemoveClipboardBlacklistTitle.text()
                            } else {
                                I18nKey::BottomBarClipboardBlacklist.text()
                            };
                            div()
                                .id("bottom-clipboard-blacklist")
                                .w(px(22.))
                                .h(px(22.))
                                .rounded(px(4.))
                                .font_family("iconfont")
                                .text_size(px(13.))
                                .text_color(base_color)
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor(CursorStyle::PointingHand)
                                .hover(|style| style.text_color(theme.accent))
                                .tooltip(move |window, cx| {
                                    let label = tooltip_label;
                                    Tooltip::element(move |_window, _cx| div().text_size(px(10.)).child(label)).build(window, cx)
                                })
                                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                    cx.stop_propagation();
                                    if is_blacklisted {
                                        settings.update(cx, |_panel, cx| cx.emit(SettingsEvent::ShowHotkeyConfirm(hotkey::HotkeyConfirmAction::RemoveClipboardBlacklist { app_name: fg.clone() })));
                                    } else {
                                        settings.update(cx, |_panel, cx| cx.emit(SettingsEvent::ShowHotkeyConfirm(hotkey::HotkeyConfirmAction::AddClipboardBlacklist { app_name: fg.clone() })));
                                    }
                                })
                                .child("\u{e638}")
                        })
                        .child({
                            let fg = fg_name.clone();
                            let app_state = app_state.clone();
                            let settings = settings.clone();
                            div()
                                .id("bottom-hotkey-blacklist")
                                .w(px(22.))
                                .h(px(22.))
                                .rounded(px(4.))
                                .font_family("iconfont")
                                .text_size(px(13.))
                                .text_color(theme.text_3)
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor(CursorStyle::PointingHand)
                                .hover(|style| style.text_color(theme.accent))
                                .tooltip(|window, cx| {
                                    let label = I18nKey::BottomBarHotkeyBlacklist.text();
                                    Tooltip::element(move |_window, _cx| div().text_size(px(10.)).child(label)).build(window, cx)
                                })
                                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                    cx.stop_propagation();
                                    if crate::core::settings::is_app_in_list(&app_state.read(cx).settings.hotkey_blacklist, &fg) {
                                        app_state.update(cx, |state, cx| { state.show_toast(I18nKey::BottomBarAlreadyInList.text()); cx.notify(); });
                                    } else {
                                        settings.update(cx, |_panel, cx| cx.emit(SettingsEvent::ShowHotkeyConfirm(hotkey::HotkeyConfirmAction::AddBlacklist { app_name: fg.clone() })));
                                    }
                                })
                                .child("\u{e600}")
                        })
                )
            })
    }
}
