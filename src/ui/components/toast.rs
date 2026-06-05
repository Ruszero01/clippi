//! Toast notification — brief overlay message that auto-dismisses.
//!
//! The toast is a lightweight, reusable component for showing transient
//! feedback messages (e.g. "No OCR text detected", "Copied to clipboard").
//! Auto-dismiss is handled by the parent view via a timer; this component
//! only renders the visual element.

use gpui::*;

use crate::ui::theme::ClippiTheme;

/// A short-lived notification message displayed at the bottom of the panel.
///
/// # Usage
///
/// ```ignore
/// // In parent render():
/// if let Some(ref msg) = toast_message {
///     Toast::new(msg)
///         .theme(theme)
///         .into_any_element()
/// }
/// ```
#[derive(IntoElement)]
pub struct Toast {
    message: String,
    theme: ClippiTheme,
    /// Optional icon glyph from iconfont (e.g. "\u{e606}" for info).
    icon: Option<String>,
}

impl Toast {
    /// Create a toast with the given message text.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            theme: ClippiTheme::dark(),
            icon: None,
        }
    }

    /// Apply a theme colour palette.
    pub fn theme(mut self, theme: ClippiTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Set an optional leading icon (iconfont glyph, 12px).
    pub fn icon(mut self, glyph: impl Into<String>) -> Self {
        self.icon = Some(glyph.into());
        self
    }
}

/// Default auto-dismiss duration for toast notifications.
pub const TOAST_DURATION: std::time::Duration = std::time::Duration::from_secs(3);

impl RenderOnce for Toast {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let bg = self.theme.toast_bg;
        let text_color = self.theme.toast_text;
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
            .gap(px(8.));

        if let Some(icon) = self.icon {
            row = row.child(
                div()
                    .font_family("iconfont")
                    .text_size(px(12.))
                    .text_color(text_color)
                    .child(icon),
            );
        }

        row = row.child(
            div()
                .text_size(px(12.))
                .text_color(text_color)
                .child(self.message),
        );

        row
    }
}
