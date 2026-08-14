//! Unified app-list popup dialog used by hotkey-blacklist, paste-shortcuts,
//! and clipboard-app-blacklist settings.  Designed as a one-shot render
//! (free function) following the same overlay pattern as
//! `render_latest_hotkeys_popup_overlay`.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::scroll::{Scrollbar, ScrollbarAxis};
use gpui_component::tooltip::Tooltip;

use crate::core::i18n_keys::I18nKey;
use crate::ui::theme::ClippiTheme;

const DIALOG_WIDTH: f32 = 304.;
const MIN_DIALOG_HEIGHT: f32 = 196.;
const MAX_DIALOG_HEIGHT: f32 = 400.;
const ENTRY_HEIGHT: f32 = 32.;
const EMPTY_LIST_HEIGHT: f32 = 64.;
const ADD_BUTTON_HEIGHT: f32 = 34.;
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
    /// Stable key for the entry list's scroll state. Must be unique per
    /// dialog instance so the three app-list popups never share or leak
    /// scroll offsets into one another.
    pub scroll_key: &'static str,
    /// App currently being recorded for (paste-shortcut mode).
    pub recording_app: Option<String>,
    /// Show the shortcut column (paste-shortcut mode).
    pub show_shortcut_column: bool,
    /// Optional help text shown from the title-bar question mark.
    pub help_text: Option<String>,
    /// Label for the bottom action button. Omit to hide.
    pub add_button_label: Option<String>,

    /// Highlight the bottom action button while shortcut recording is active.
    pub add_button_recording: bool,
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

/// Scrollable entry list with a per-dialog scroll-state key.
///
/// Mirrors `gpui_component::scroll::Scrollable` (tracked scroll handle +
/// overlay scrollbar), but keys the `ScrollHandle` by an explicit name
/// instead of the call-site location, so each app-list popup keeps its own
/// scroll state and never inherits another popup's offset.
#[derive(IntoElement)]
struct AppListScrollArea {
    key: &'static str,
    children: Vec<AnyElement>,
}

impl RenderOnce for AppListScrollArea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let key = ElementId::Name(self.key.into());
        let scroll_handle = window
            .use_keyed_state(key.clone(), cx, |_, _| ScrollHandle::default())
            .read(cx)
            .clone();
        div()
            .id(key)
            .size_full()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .relative()
            .child(
                div()
                    .id("app-list-scroll-area")
                    .flex()
                    .flex_col()
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&scroll_handle)
                    .children(self.children),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .child(
                        Scrollbar::new(&scroll_handle)
                            .id("app-list-scrollbar")
                            .axis(ScrollbarAxis::Vertical),
                    ),
            )
    }
}

/// Render the app-list dialog overlay (one-shot, not a persistent Entity).
pub fn render_app_list_dialog(params: AppListDialogParams) -> impl IntoElement {
    let (opacity, scale, offset, viewport_width, viewport_height) = params.layout;
    let popup_width = DIALOG_WIDTH * scale;
    let entry_count = params.entries.len();
    let visible_entries = entry_count.min(MAX_VISIBLE_ENTRIES);
    let has_add = params.add_button_label.is_some() && params.on_add.is_some();
    let list_content_height = if entry_count == 0 {
        EMPTY_LIST_HEIGHT
    } else {
        visible_entries as f32 * ENTRY_HEIGHT
    };
    let add_section_height = if has_add { ADD_BUTTON_HEIGHT + 10. } else { 0. };
    let popup_height = ((72. + list_content_height + add_section_height) * scale)
        .clamp(MIN_DIALOG_HEIGHT * scale, MAX_DIALOG_HEIGHT * scale);
    let main_width = (viewport_width - 36.).max(popup_width);
    let popup_left = ((main_width - popup_width) * 0.5).max(8.);
    let popup_top = ((viewport_height - popup_height) * 0.5).max(8.) + offset;
    let close = params.on_close.clone();
    let theme = params.theme.clone();
    let surface = theme.surface;
    let divider = theme.divider;
    let accent = theme.accent;
    let accent_soft = theme.accent_soft;
    let accent_hover = theme.accent_overlay();
    let text_2 = theme.text_2;
    let text_3 = theme.text_3;
    let add_button_recording = params.add_button_recording;
    let add_button_bg = if add_button_recording {
        accent_soft
    } else {
        theme.btn_hover
    };
    let add_button_border = if add_button_recording {
        accent
    } else {
        divider
    };
    let add_button_text = if add_button_recording { accent } else { text_2 };
    let add_button_hover = if add_button_recording {
        accent_hover
    } else {
        theme.surface_press
    };

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
                .h(px(popup_height))
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
                    params.help_text.clone(),
                    close.clone(),
                ))
                .child(div().h(px(1.)).bg(divider))
                // ── Entry list or empty hint ──
                .child(AppListScrollArea {
                    key: params.scroll_key,
                    children: if entry_count == 0 {
                        vec![div()
                            .flex_1()
                            .min_h(px(EMPTY_LIST_HEIGHT * scale))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.))
                            .text_color(text_3)
                            .child(params.empty_hint.clone())
                            .into_any_element()]
                    } else {
                        params
                            .entries
                            .iter()
                            .map(|entry| {
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
                                .into_any_element()
                            })
                            .collect()
                    },
                })
                // ── Add button ──
                .when_some(
                    params
                        .on_add
                        .clone()
                        .zip(params.add_button_label.clone())
                        .filter(|_| has_add),
                    |card, (on_add, btn_text)| {
                        card.child(div().h(px(1.)).bg(divider)).child(
                            div()
                                .h(px(ADD_BUTTON_HEIGHT * scale))
                                .rounded(px(7.))
                                .bg(add_button_bg)
                                .border(px(1.))
                                .border_color(add_button_border)
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(px(6.))
                                .text_size(px(11.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(add_button_text)
                                .cursor(CursorStyle::PointingHand)
                                .hover(move |style| style.bg(add_button_hover))
                                .on_mouse_down(MouseButton::Left, {
                                    let on_add = on_add.clone();
                                    move |_ev, window, cx| {
                                        cx.stop_propagation();
                                        on_add(window, cx)
                                    }
                                })
                                .child(btn_text.clone()),
                        )
                    },
                ),
        )
}

fn render_title_bar(
    title: &str,
    count: usize,
    theme: &ClippiTheme,
    scale: f32,
    help_text: Option<String>,
    on_close: AppListAction,
) -> impl IntoElement {
    let accent = theme.accent;
    let divider = theme.divider;
    let btn_hover = theme.btn_hover;
    let text_1 = theme.text_1;
    let text_2 = theme.text_2;
    let help_lines = help_text.map(|text| text.lines().map(str::to_owned).collect::<Vec<_>>());

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
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .when_some(help_lines, |actions, lines| {
                    let tooltip_lines = lines.clone();
                    actions.child(
                        div()
                            .id("app-list-help")
                            .w(px(20.))
                            .h(px(20.))
                            .rounded(px(10.))
                            .border(px(1.))
                            .border_color(divider)
                            .font_family("iconfont")
                            .text_size(px(13.))
                            .text_color(text_2)
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(move |style| style.bg(btn_hover).text_color(accent))
                            .tooltip(move |window, cx| {
                                let tooltip_lines = tooltip_lines.clone();
                                Tooltip::element(move |_window, _cx| {
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(3.))
                                        .text_size(px(10.))
                                        .children(
                                            tooltip_lines
                                                .clone()
                                                .into_iter()
                                                .map(|line| div().whitespace_nowrap().child(line)),
                                        )
                                })
                                .build(window, cx)
                            })
                            .child("\u{e60a}"),
                    )
                })
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
                }),
        )
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
                .flex_1()
                .min_w(px(0.))
                .child(
                    gpui::img(std::path::Path::new(&icon_path))
                        .w(px(18. * scale))
                        .h(px(18. * scale)),
                )
                .text_size(px(11.))
                .text_color(text_1)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(name_display),
                ),
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
                                .child(I18nKey::HotkeyPasteShortcutRecording.text()),
                        )
                    } else {
                        let label = entry
                            .shortcut
                            .as_deref()
                            .map(crate::platform::hotkey::hotkey_display)
                            .unwrap_or_default();
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
                        .child("\u{e696}")
                }),
        )
}
