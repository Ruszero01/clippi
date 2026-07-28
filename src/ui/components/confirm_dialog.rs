//! Confirm dialog — modal overlay for destructive action confirmation.
//!
//! Reusable confirmation dialog with full-window backdrop, centered card,
//! and cancel/confirm buttons. Supports danger styling for destructive actions.
//! Used for delete confirmations (single/batch) and hotkey blacklist removal.
//!
//! --- # Usage ---
//!
//! --- ```ignore ---
//! --- ConfirmDialog::delete_single() ---
//! --- .on_confirm(|_window, cx| { /* delete logic */ }) ---
//! --- .on_cancel(|_window, cx| { /* dismiss */ }) ---
//! --- .render_animated(window, cx, generation) // generation: u64, bump on each show ---
//! --- ``` ---

use std::rc::Rc;
use std::time::Duration;

use crate::core::i18n_keys::I18nKey;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_transitions::WindowUseTransition;

use crate::ui::theme::ClippiTheme;

type DialogHandler = Rc<dyn Fn(&mut Window, &mut App)>;

struct DialogOption {
    label: String,
    selected: bool,
    on_toggle: DialogHandler,
}

const DIALOG_ANIM_DURATION: Duration = Duration::from_millis(150);

// --- Component ---

pub struct ConfirmDialog {
    title: String,
    message: String,
    confirm_label: String,
    cancel_label: String,
    danger: bool,
    theme: ClippiTheme,
    on_confirm: Option<DialogHandler>,
    on_cancel: Option<DialogHandler>,
    option: Option<DialogOption>,
    focus_handle: Option<FocusHandle>,
}

impl ConfirmDialog {
    pub fn new() -> Self {
        Self {
            title: String::new(),
            message: String::new(),
            confirm_label: I18nKey::DialogConfirm.text().into(),
            cancel_label: I18nKey::BtnCancel.text().into(),
            danger: false,
            theme: ClippiTheme::dark(),
            on_confirm: None,
            on_cancel: None,
            option: None,
            focus_handle: None,
        }
    }

    // --- Builder methods ---

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    pub fn confirm_label(mut self, label: impl Into<String>) -> Self {
        self.confirm_label = label.into();
        self
    }

    pub fn danger(mut self, danger: bool) -> Self {
        self.danger = danger;
        self
    }

    pub fn theme(mut self, theme: ClippiTheme) -> Self {
        self.theme = theme;
        self
    }

    pub fn on_confirm(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_confirm = Some(Rc::new(handler));
        self
    }

    pub fn on_cancel(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(Rc::new(handler));
        self
    }

    /// Add a single radio-style boolean option between the message and actions.
    pub fn option(
        mut self,
        label: impl Into<String>,
        selected: bool,
        on_toggle: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.option = Some(DialogOption {
            label: label.into(),
            selected,
            on_toggle: Rc::new(on_toggle),
        });
        self
    }

    /// Set a focus handle so keyboard events (Enter/Esc) reach the dialog.
    pub fn focus_handle(mut self, handle: FocusHandle) -> Self {
        self.focus_handle = Some(handle);
        self
    }

    // --- Preset factory methods ---

    /// Single item delete confirmation — simple yes/no.
    pub fn delete_single() -> Self {
        Self::new()
            .title(I18nKey::ConfirmDeleteSingleTitle.text())
            .message(I18nKey::ConfirmDeleteSingleMsg.text())
            .confirm_label(I18nKey::ConfirmDeleteLabel.text())
            .danger(true)
    }

    /// Batch delete confirmation for N selected items.
    pub fn delete_batch(count: usize) -> Self {
        Self::new()
            .title(I18nKey::ConfirmBatchTitle.text())
            .message(I18nKey::ConfirmBatchMsg.fmt(&[&count.to_string()]))
            .confirm_label(I18nKey::ConfirmDeleteLabel.text())
            .danger(true)
    }

    /// Remove app from hotkey blacklist confirmation.
    pub fn remove_blacklist(app_name: &str) -> Self {
        Self::new()
            .title(I18nKey::ConfirmRemoveTitle.text())
            .message(I18nKey::ConfirmRemoveMsg.fmt(&[app_name]))
            .confirm_label(I18nKey::ConfirmRemoveLabel.text())
            .danger(false)
    }

    /// Add app to hotkey blacklist confirmation.
    pub fn add_blacklist(app_name: &str) -> Self {
        Self::new()
            .title(I18nKey::ConfirmAddBlacklistTitle.text())
            .message(I18nKey::ConfirmAddBlacklistMsg.fmt(&[app_name]))
            .confirm_label(I18nKey::ConfirmAddLabel.text())
            .danger(false)
    }

    /// Add paste shortcut confirmation.
    pub fn add_paste_shortcut(app_name: &str, shortcut: &str) -> Self {
        Self::new()
            .title(I18nKey::HotkeyPasteShortcutConfirmAddTitle.text())
            .message(I18nKey::HotkeyPasteShortcutConfirmAddMsg.fmt(&[app_name, shortcut]))
            .confirm_label(I18nKey::ConfirmAddLabel.text())
            .danger(false)
    }

    /// Remove paste shortcut confirmation.
    pub fn remove_paste_shortcut(app_name: &str) -> Self {
        Self::new()
            .title(I18nKey::HotkeyPasteShortcutConfirmRemoveTitle.text())
            .message(I18nKey::HotkeyPasteShortcutConfirmRemoveMsg.fmt(&[app_name]))
            .confirm_label(I18nKey::ConfirmRemoveLabel.text())
            .danger(false)
    }

    /// Confirmation before adding an app to the clipboard app blacklist.
    pub fn add_clipboard_blacklist(app_name: &str) -> Self {
        Self::new()
            .title(I18nKey::ConfirmAddClipboardBlacklistTitle.text())
            .message(I18nKey::ConfirmAddClipboardBlacklistMsg.fmt(&[app_name]))
            .confirm_label(I18nKey::ConfirmAddLabel.text())
            .danger(false)
    }

    /// Confirmation before removing an app from the clipboard app blacklist.
    pub fn remove_clipboard_blacklist(app_name: &str) -> Self {
        Self::new()
            .title(I18nKey::ConfirmRemoveClipboardBlacklistTitle.text())
            .message(I18nKey::ConfirmRemoveClipboardBlacklistMsg.fmt(&[app_name]))
            .confirm_label(I18nKey::ConfirmRemoveLabel.text())
            .danger(false)
    }
}

impl ConfirmDialog {
    /// Render the dialog with built-in enter animation (opacity + scale).
    ///
    /// `generation` should be bumped each time the dialog appears, so the
    /// animation restarts.  Use 0 to skip animation (dialog renders at full
    /// opacity / scale 1.0 immediately).
    pub fn render_animated(self, window: &mut Window, cx: &mut App, generation: u64) -> AnyElement {
        let Self {
            title,
            message,
            confirm_label,
            cancel_label,
            danger: is_danger,
            theme,
            on_confirm,
            on_cancel,
            option,
            focus_handle,
        } = self;

        let animating = generation != 0;
        let (opacity, scale) = if animating {
            let t_op = window.use_keyed_transition(
                ("cd-opacity", generation),
                cx,
                DIALOG_ANIM_DURATION,
                move |_, _| 0.0,
            );
            t_op.update(cx, |v, cx| {
                *v = 1.0;
                cx.notify();
            });
            let op = *t_op.evaluate(window, cx);
            let t_sc = window.use_keyed_transition(
                ("cd-scale", generation),
                cx,
                DIALOG_ANIM_DURATION,
                move |_, _| 0.96,
            );
            t_sc.update(cx, |v, cx| {
                *v = 1.0;
                cx.notify();
            });
            let sc = *t_sc.evaluate(window, cx);
            (op, sc)
        } else {
            (1.0, 1.0)
        };

        let confirm_color = if is_danger {
            theme.danger
        } else {
            theme.accent
        };

        if let Some(ref handle) = focus_handle {
            handle.focus(window);
        }

        div()
            .absolute()
            .size_full()
            .bg(rgba(0x00000033))
            .rounded(px(12.))
            .flex()
            .items_center()
            .justify_center()
            .opacity(opacity)
            .when_some(focus_handle.as_ref(), |d, handle| d.track_focus(handle))
            .on_key_down({
                let on_confirm = on_confirm.clone();
                let on_cancel = on_cancel.clone();
                move |ev: &KeyDownEvent, window, cx| match ev.keystroke.key.as_str() {
                    "escape" => {
                        cx.stop_propagation();
                        if let Some(ref handler) = on_cancel {
                            handler(window, cx);
                        }
                    }
                    "enter" => {
                        cx.stop_propagation();
                        if let Some(ref handler) = on_confirm {
                            handler(window, cx);
                        }
                    }
                    _ => {}
                }
            })
            .on_mouse_down(MouseButton::Left, {
                let on_cancel = on_cancel.clone();
                move |_ev, _window, cx| {
                    cx.stop_propagation();
                    if let Some(ref handler) = on_cancel {
                        handler(_window, cx);
                    }
                }
            })
            .child(
                div()
                    .w(px(280. * scale))
                    .bg(theme.panel_surface)
                    .rounded(px(12.))
                    .border(px(1.))
                    .border_color(theme.panel_sep_line)
                    .p(px(16. * scale))
                    .occlude()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_1)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.text_2)
                            .mt(px(8.))
                            .child(message),
                    )
                    .when_some(option, |dialog, option| {
                        let on_toggle = option.on_toggle.clone();
                        dialog.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .mt(px(12.))
                                .py(px(4.))
                                .cursor(CursorStyle::PointingHand)
                                .hover({
                                    let hover_bg = theme.btn_hover;
                                    move |row| row.rounded(px(4.)).bg(hover_bg)
                                })
                                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                    cx.stop_propagation();
                                    on_toggle(window, cx);
                                })
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .font_family("iconfont")
                                        .text_size(px(12.))
                                        .text_color(if option.selected {
                                            theme.accent
                                        } else {
                                            theme.text_3
                                        })
                                        .child(if option.selected {
                                            "\u{e61f}"
                                        } else {
                                            "\u{e831}"
                                        }),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(theme.text_1)
                                        .child(option.label),
                                ),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.))
                            .mt(px(16.))
                            .child({
                                let label = cancel_label.clone();
                                let on_cancel = on_cancel.clone();
                                div()
                                    .h(px(24.))
                                    .px(px(12.))
                                    .rounded(px(4.))
                                    .text_size(px(12.))
                                    .text_color(theme.text_2)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover({
                                        let hover_bg = theme.btn_hover;
                                        move |style| style.bg(hover_bg)
                                    })
                                    .on_mouse_down(MouseButton::Left, {
                                        let on_cancel = on_cancel.clone();
                                        move |_ev, _window, cx| {
                                            cx.stop_propagation();
                                            if let Some(ref handler) = on_cancel {
                                                handler(_window, cx);
                                            }
                                        }
                                    })
                                    .child(label)
                            })
                            .child({
                                let label = confirm_label.clone();
                                let on_confirm = on_confirm.clone();
                                div()
                                    .h(px(24.))
                                    .px(px(12.))
                                    .rounded(px(4.))
                                    .text_size(px(12.))
                                    .text_color(rgb(0xffffff))
                                    .bg(confirm_color)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(move |style| style.opacity(0.85))
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                        cx.stop_propagation();
                                        if let Some(ref handler) = on_confirm {
                                            handler(_window, cx);
                                        }
                                    })
                                    .child(label)
                            }),
                    ),
            )
            .into_any_element()
    }
}
