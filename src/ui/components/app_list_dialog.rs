//! Unified app-list popup dialog used by hotkey-blacklist, paste-shortcuts,
//! and clipboard-app-blacklist settings.  Designed as a one-shot render
//! (free function) following the same overlay pattern as
//! `render_latest_hotkeys_popup_overlay`.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::scroll::ScrollableElement;

use crate::core::i18n_keys::I18nKey;
use crate::ui::theme::ClippiTheme;

const DIALOG_WIDTH: f32 = 304.;
const ENTRY_HEIGHT: f32 = 32.;
const MAX_VISIBLE_ENTRIES: usize = 8;
pub type AppListAction = Rc<dyn Fn(&mut Window, &mut App)>;
pub type AppListNameAction = Rc<dyn Fn(String, &mut Window, &mut App)>;

struct AppListEntryActions<'a> {
    recording_app: Option<&'a str>,
    on_delete: &'a AppListNameAction,
    on_shortcut_click: &'a Option<AppListNameAction>,
    on_cancel_recording: &'a Option<AppListAction>,
}

/// A single entry in the app-list dialog.
pub struct AppListDialogEntry {
    pub app_name: String,
    /// Only populated for paste-shortcut entries.
    pub shortcut: Option<String>,
    /// Highlight this entry with an accent border (recording target).
    pub is_recording_target: bool,
}

/// Parameters for rendering the unified app-list popup.
pub struct AppListDialogParams {
    pub title: String,
    pub empty_hint: String,
    pub entries: Vec<AppListDialogEntry>,
    /// App currently being recorded for (paste-shortcut mode).
    pub recording_app: Option<String>,
    /// Show the shortcut column (paste-shortcut mode).
    pub show_shortcut_column: bool,
    /// Label for the bottom "add current app" button.  Omit to hide.
    pub add_button_label: Option<String>,
    pub theme: ClippiTheme,
    /// (opacity, scale, offset, viewport_w, viewport_h)
    pub layout: (f32, f32, f32, f32, f32),
    pub on_close: AppListAction,
    /// Called when the delete button next to `app_name` is clicked.
    pub on_delete: AppListNameAction,
    /// Called when the shortcut label is clicked (paste-shortcut mode).
    pub on_shortcut_click: Option<AppListNameAction>,
    /// Called when the cancel button on a recording entry is clicked.
    pub on_cancel_recording: Option<AppListAction>,
    /// Called when the bottom "+ 添加当前应用" button is clicked.
    pub on_add: Option<AppListAction>,
}

/// Render the app-list dialog overlay (one-shot, not a persistent Entity).
pub fn render_app_list_dialog(params: AppListDialogParams) -> impl IntoElement {
    let (opacity, scale, offset, viewport_width, viewport_height) = params.layout;
    let popup_width = DIALOG_WIDTH * scale;
    let entry_count = params.entries.len();
    let visible_entries = entry_count.min(MAX_VISIBLE_ENTRIES);
    let list_height = visible_entries.max(1) as f32 * ENTRY_HEIGHT * scale;
    let has_add = params.add_button_label.is_some() && params.on_add.is_some();
    let add_btn_height: f32 = if has_add { 36. * scale } else { 0. };
    let popup_height = (60. + list_height + add_btn_height).min(400. * scale);
    let main_width = (viewport_width - 36.).max(popup_width);
    let popup_left = ((main_width - popup_width) * 0.5).max(8.);
    let popup_top = ((viewport_height - popup_height) * 0.5).max(8.) + offset;
    let close = params.on_close.clone();
    let theme = params.theme.clone();
    let surface = theme.surface;
    let divider = theme.divider;
    let accent = theme.accent;
    let text_3 = theme.text_3;

    div()
        .absolute()
        .left(px(36.))
        .right(px(0.))
        .top(px(0.))
        .bottom(px(0.))
        .bg(rgba(0x00000033))
        .rounded(px(12.))
        .opacity(opacity)
        .cursor(CursorStyle::PointingHand)
        .on_mouse_down(MouseButton::Left, {
            let close = close.clone();
            move |_ev, window, cx| close(window, cx)
        })
        .child(
            div()
                .absolute()
                .left(px(popup_left))
                .top(px(popup_top))
                .w(px(popup_width))
                .max_w(px(popup_width))
                .rounded(px(8.))
                .bg(surface)
                .border(px(1.))
                .border_color(divider)
                .shadow_lg()
                .p(px(12. * scale))
                .occlude()
                .flex()
                .flex_col()
                .gap(px(8.))
                .cursor(CursorStyle::Arrow)
                .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
                    cx.stop_propagation();
                })
                // ── Title bar ──
                .child(render_title_bar(
                    &params.title,
                    entry_count,
                    &theme,
                    scale,
                    close.clone(),
                ))
                .child(div().h(px(1.)).bg(divider))
                // ── Entry list or empty hint ──
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .max_h(px(list_height))
                        .overflow_y_scrollbar()
                        .when(entry_count == 0, |list| {
                            list.child(
                                div()
                                    .h(px(40.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(11.))
                                    .text_color(text_3)
                                    .child(params.empty_hint.clone()),
                            )
                        })
                        .children(params.entries.iter().map(|entry| {
                            render_entry(
                                entry,
                                params.show_shortcut_column,
                                AppListEntryActions {
                                    recording_app: params.recording_app.as_deref(),
                                    on_delete: &params.on_delete,
                                    on_shortcut_click: &params.on_shortcut_click,
                                    on_cancel_recording: &params.on_cancel_recording,
                                },
                                &theme,
                                scale,
                            )
                        })),
                )
                // ── Add button ──
                .when(has_add, |card| {
                    let on_add = params.on_add.clone().unwrap();
                    let btn_text = params.add_button_label.clone().unwrap();
                    card.child(div().h(px(1.)).bg(divider)).child(
                        div()
                            .h(px(30.))
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(4.))
                            .text_size(px(11.))
                            .text_color(accent)
                            .cursor(CursorStyle::PointingHand)
                            .hover(|style| style.bg(rgba(0xffffff0d)))
                            .on_mouse_down(MouseButton::Left, {
                                let on_add = on_add.clone();
                                move |_ev, window, cx| {
                                    cx.stop_propagation();
                                    on_add(window, cx)
                                }
                            })
                            .child(
                                div()
                                    .font_family("iconfont")
                                    .text_size(px(12.))
                                    .child("\u{e618}"),
                            )
                            .child(btn_text.clone()),
                    )
                }),
        )
}

fn render_title_bar(
    title: &str,
    count: usize,
    theme: &ClippiTheme,
    scale: f32,
    on_close: AppListAction,
) -> impl IntoElement {
    let accent = theme.accent;
    let text_1 = theme.text_1;
    let text_2 = theme.text_2;

    div()
        .h(px(28.))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .flex_1()
                .items_center()
                .gap(px(14.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(text_1)
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .h(px(22.))
                        .px(px(8. * scale))
                        .rounded(px(11.))
                        .bg(theme.accent_soft)
                        .border(px(1.))
                        .border_color(accent)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(10.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(accent)
                        .child(I18nKey::ClipboardAppBlacklistCount.fmt(&[&count.to_string()])),
                ),
        )
        .child({
            let close = on_close;
            div()
                .w(px(26.))
                .h(px(26.))
                .rounded(px(6.))
                .font_family("iconfont")
                .text_size(px(13.))
                .text_color(text_2)
                .flex()
                .items_center()
                .justify_center()
                .cursor(CursorStyle::PointingHand)
                .hover(|style| style.bg(rgba(0xffffff0d)))
                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                    cx.stop_propagation();
                    close(window, cx);
                })
                .child("\u{e7b7}")
        })
}

fn render_entry(
    entry: &AppListDialogEntry,
    show_shortcut: bool,
    actions: AppListEntryActions<'_>,
    theme: &ClippiTheme,
    scale: f32,
) -> impl IntoElement {
    let app_name = entry.app_name.clone();
    let icon_path = crate::core::paths::app_icon_path(&entry.app_name);
    let name_display = entry.app_name.clone();
    let is_recording = entry.is_recording_target
        || actions
            .recording_app
            .map(|name| name.eq_ignore_ascii_case(&entry.app_name))
            .unwrap_or(false);
    let text_1 = theme.text_1;
    let text_2 = theme.text_2;
    let text_3 = theme.text_3;
    let danger = theme.danger;
    let accent = theme.accent;

    div()
        .h(px(ENTRY_HEIGHT * scale))
        .rounded(px(4.))
        .px(px(8. * scale))
        .flex()
        .items_center()
        .justify_between()
        .when(is_recording, |row| row.border(px(1.)).border_color(accent))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6. * scale))
                .child(
                    gpui::img(std::path::Path::new(&icon_path))
                        .w(px(18. * scale))
                        .h(px(18. * scale)),
                )
                .text_size(px(11.))
                .text_color(text_1)
                .child(name_display),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4. * scale))
                // Shortcut label (paste-shortcut mode)
                .when(show_shortcut, |row| {
                    let next_name = app_name.clone();
                    if is_recording {
                        row.child(
                            div()
                                .h(px(20.))
                                .px(px(8. * scale))
                                .rounded(px(4.))
                                .bg(theme.accent_soft)
                                .text_size(px(10.))
                                .text_color(accent)
                                .flex()
                                .items_center()
                                .child(I18nKey::BottomBarRecording.text()),
                        )
                    } else {
                        let label = entry.shortcut.clone().unwrap_or_default();
                        let click = actions.on_shortcut_click.clone();
                        let label_name = next_name;
                        row.child(
                            div()
                                .h(px(20.))
                                .px(px(8. * scale))
                                .rounded(px(4.))
                                .bg(theme.accent_soft)
                                .text_size(px(10.))
                                .text_color(accent)
                                .cursor(CursorStyle::PointingHand)
                                .on_mouse_down(MouseButton::Left, {
                                    move |_ev, window, cx| {
                                        cx.stop_propagation();
                                        if let Some(ref click) = click {
                                            click(label_name.clone(), window, cx);
                                        }
                                    }
                                })
                                .child(label.clone()),
                        )
                    }
                })
                // Cancel button (when recording)
                .when(is_recording, |row| {
                    let cancel = actions.on_cancel_recording.clone();
                    row.child(
                        div()
                            .w(px(20.))
                            .h(px(20.))
                            .rounded(px(4.))
                            .font_family("iconfont")
                            .text_size(px(12.))
                            .text_color(text_2)
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(|style| style.bg(rgba(0xffffff0d)))
                            .on_mouse_down(MouseButton::Left, {
                                move |_ev, window, cx| {
                                    cx.stop_propagation();
                                    if let Some(ref cancel) = cancel {
                                        cancel(window, cx);
                                    }
                                }
                            })
                            .child("\u{e7b7}"),
                    )
                })
                // Delete button
                .child({
                    let del_name = app_name.clone();
                    let del_cb = actions.on_delete.clone();
                    div()
                        .w(px(20.))
                        .h(px(20.))
                        .rounded(px(4.))
                        .font_family("iconfont")
                        .text_size(px(12.))
                        .text_color(text_3)
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor(CursorStyle::PointingHand)
                        .hover(|style| style.text_color(danger))
                        .on_mouse_down(MouseButton::Left, {
                            let del_name = del_name.clone();
                            let del_cb = del_cb.clone();
                            move |_ev, window, cx| {
                                cx.stop_propagation();
                                del_cb(del_name.clone(), window, cx);
                            }
                        })
                        .child("\u{e8b6}")
                }),
        )
}
