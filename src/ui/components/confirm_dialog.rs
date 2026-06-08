//! Confirm dialog — modal overlay for destructive action confirmation.
//!
//! Reusable confirmation dialog with full-window backdrop, centered card,
//! and cancel/confirm buttons. Supports danger styling for destructive actions.
//! Used for delete confirmations (single/batch) and hotkey blacklist removal.
//!
//! --- # Usage ---
//!
//! --- ```ignore ---
//! --- ConfirmDialog::delete_single("example text") ---
//! --- .on_confirm(|_window, cx| { /* delete logic */ }) ---
//! --- .on_cancel(|_window, cx| { /* dismiss */ }) ---
//! --- .into_any_element() ---
//! --- ``` ---

use std::rc::Rc;

use gpui::*;
use crate::core::i18n_keys::I18nKey;

use crate::ui::theme::ClippiTheme;

type DialogHandler = Rc<dyn Fn(&mut Window, &mut App)>;

// --- Component ---

#[derive(IntoElement)]
pub struct ConfirmDialog {
    title: String,
    message: String,
    confirm_label: String,
    cancel_label: String,
    danger: bool,
    theme: ClippiTheme,
    on_confirm: Option<DialogHandler>,
    on_cancel: Option<DialogHandler>,
}

impl ConfirmDialog {
    pub fn new() -> Self {
        Self {
            title: String::new(),
            message: String::new(),
            confirm_label: "Confirm".into(),
            cancel_label: "Cancel".into(),
            danger: false,
            theme: ClippiTheme::dark(),
            on_confirm: None,
            on_cancel: None,
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
}

impl RenderOnce for ConfirmDialog {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Self {
            title,
            message,
            confirm_label,
            cancel_label,
            danger: is_danger,
            theme,
            on_confirm,
            on_cancel,
        } = self;

        let confirm_color = if is_danger {
            theme.danger
        } else {
            theme.accent
        };

        // --- Transparent overlay (covers parent) + centered modal card. ---
        // --- Backdrop is fully transparent — the user sees through to the ---
        // --- panel content below. Clicking the backdrop cancels the dialog. ---
        div()
            .absolute()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
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
            // --- Backdrop click → cancel ---
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
                // --- Modal card — occluded to prevent click-through to backdrop ---
                div()
                    .w(px(280.))
                    .bg(theme.panel_surface)
                    .rounded(px(12.))
                    .border(px(1.))
                    .border_color(theme.panel_sep_line)
                    .p(px(16.))
                    .occlude()
                    // --- Title — 14px bold, text_1 ---
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_1)
                            .child(title),
                    )
                    // --- Message — 12px, text_2, 8px top margin ---
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.text_2)
                            .mt(px(8.))
                            .child(message),
                    )
                    // --- Button row — flex row, justify_end, 8px gap, 16px top margin ---
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.))
                            .mt(px(16.))
                            // --- Cancel button ---
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
                            // --- Confirm button ---
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
    }
}
