//! Toast notification — brief overlay message that auto-dismisses.
//!
//! The toast is a lightweight, reusable component for showing transient
//! feedback messages (e.g. "No OCR text detected", "Copied to clipboard").
//! Auto-dismiss is handled by the parent view via a timer; this component
//! --- only renders the visual element. ---
//!
//! Action buttons can be added to toasts for interactive notifications
//! (e.g. "Download" / "Later" for update notifications).

use std::rc::Rc;

use gpui::*;

use crate::ui::theme::ClippiTheme;

type ToastClickHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// An action button shown on a toast.
pub struct ToastAction {
    pub label: String,
    pub on_click: ToastClickHandler,
    /// Primary buttons get filled accent background; secondary are outlined.
    pub primary: bool,
}

impl Clone for ToastAction {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            on_click: Rc::clone(&self.on_click),
            primary: self.primary,
        }
    }
}

#[derive(IntoElement)]
pub struct Toast {
    message: String,
    theme: ClippiTheme,
    icon: Option<String>,
    actions: Vec<ToastAction>,
}

impl Toast {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            theme: ClippiTheme::dark(),
            icon: None,
            actions: Vec::new(),
        }
    }

    pub fn theme(mut self, theme: ClippiTheme) -> Self {
        self.theme = theme;
        self
    }

    pub fn actions(mut self, actions: Vec<ToastAction>) -> Self {
        self.actions = actions;
        self
    }
}

/// Default auto-dismiss duration for toast notifications.
pub const TOAST_DURATION: std::time::Duration = std::time::Duration::from_secs(3);

impl RenderOnce for Toast {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let bg = self.theme.toast_bg;
        let text_color = self.theme.accent;
        let border_color = self.theme.surface_press;

        let mut row = div()
            .w_full()
            .px(px(16.))
            .py(px(10.))
            .bg(bg)
            .rounded(px(8.))
            .border_color(border_color)
            .border(px(1.))
            .shadow_md()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.));

        // Left: icon + message
        let mut left = div().flex().flex_row().items_center().gap(px(8.));

        if let Some(icon) = self.icon {
            left = left.child(
                div()
                    .font_family("iconfont")
                    .text_size(px(12.))
                    .text_color(text_color)
                    .child(icon),
            );
        }

        left = left.child(
            div()
                .text_size(px(12.))
                .text_color(text_color)
                .child(self.message),
        );

        row = row.child(left);

        // Right: action buttons (matching ConfirmDialog button style)
        if !self.actions.is_empty() {
            let accent = self.theme.accent;
            let btn_hover = self.theme.btn_hover;
            let text_2 = self.theme.text_2;
            let buttons =
                div()
                    .flex()
                    .flex_row()
                    .gap(px(6.))
                    .children(self.actions.into_iter().map(move |action| {
                        let is_primary = action.primary;
                        let btn_bg = if is_primary { accent } else { rgba(0x00000000) };
                        let btn_text = if is_primary { rgb(0xffffff) } else { text_2 };
                        div()
                            .h(px(22.))
                            .px(px(10.))
                            .rounded(px(4.))
                            .bg(btn_bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(move |style| {
                                style.bg(if is_primary { accent } else { btn_hover })
                            })
                            .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                (action.on_click)(window, cx);
                            })
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(btn_text)
                                    .child(action.label),
                            )
                    }));
            row = row.child(buttons);
        }

        row
    }
}
