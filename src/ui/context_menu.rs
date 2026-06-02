//! Context menu — right-click menu for clipboard items.
//!
//! Matches the original Slint ContextMenu.slint design:
//! - 164px width, 8px border-radius, 4px padding
//! - 30px item height, 5px border-radius per item
//! - Icon (13px) + label (13px) per row
//! - Separators: 3px gap + 1px line + 3px gap
//! - Single and batch mode variants

use std::rc::Rc;

use gpui::*;

/// Describes a single menu item.
pub struct MenuItem {
    pub label: String,
    pub action: String,
    pub icon: String,
    pub danger: bool,
    pub fav: bool,
}

/// Separator marker between menu groups.
const SEPARATOR: &str = "__sep__";

#[derive(IntoElement)]
pub struct ContextMenu {
    items: Vec<MenuItem>,
    dark_mode: bool,
    on_action: Option<Rc<dyn Fn(&str, &mut Window, &mut App)>>,
}

impl ContextMenu {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            dark_mode: true,
            on_action: None,
        }
    }

    /// Build a single-item context menu.
    pub fn single_item() -> Self {
        Self::new().items(vec![
            // Copy
            MenuItem { label: "Copy".into(), action: "copy".into(), icon: "\u{1F4CB}".into(), danger: false, fav: false },
            // Paste
            MenuItem { label: "Paste".into(), action: "paste".into(), icon: "\u{1F4CB}".into(), danger: false, fav: false },
            // Separator
            MenuItem { label: SEPARATOR.into(), action: String::new(), icon: String::new(), danger: false, fav: false },
            // Edit
            MenuItem { label: "Edit".into(), action: "edit".into(), icon: "\u{270F}".into(), danger: false, fav: false },
            // Note
            MenuItem { label: "Note".into(), action: "edit_note".into(), icon: "\u{1F4DD}".into(), danger: false, fav: false },
            // Tag
            MenuItem { label: "Tag".into(), action: "tag".into(), icon: "\u{1F3F7}".into(), danger: false, fav: false },
            // Separator
            MenuItem { label: SEPARATOR.into(), action: String::new(), icon: String::new(), danger: false, fav: false },
            // Favorite
            MenuItem { label: "Favorite".into(), action: "favorite".into(), icon: "\u{2605}".into(), danger: false, fav: true },
            // Delete
            MenuItem { label: "Delete".into(), action: "delete".into(), icon: "\u{1F5D1}".into(), danger: true, fav: false },
        ])
    }

    /// Build a batch context menu (for multi-select).
    pub fn batch(count: usize) -> Self {
        Self::new().items(vec![
            // Batch paste
            MenuItem { label: format!("Paste {} items", count), action: "batch_paste".into(), icon: "\u{1F4CB}".into(), danger: false, fav: false },
            // Separator
            MenuItem { label: SEPARATOR.into(), action: String::new(), icon: String::new(), danger: false, fav: false },
            // Batch tag
            MenuItem { label: "Batch tag".into(), action: "batch_tag".into(), icon: "\u{1F3F7}".into(), danger: false, fav: false },
            // Separator
            MenuItem { label: SEPARATOR.into(), action: String::new(), icon: String::new(), danger: false, fav: false },
            // Batch favorite
            MenuItem { label: "Batch fav".into(), action: "batch_fav".into(), icon: "\u{2605}".into(), danger: false, fav: true },
            // Batch delete
            MenuItem { label: "Batch delete".into(), action: "batch_delete".into(), icon: "\u{1F5D1}".into(), danger: true, fav: false },
        ])
    }

    pub fn items(mut self, items: Vec<MenuItem>) -> Self {
        self.items = items;
        self
    }

    pub fn on_action(mut self, handler: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_action = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let on_action = self.on_action;
        let dark = self.dark_mode;

        // Dark theme colors (matching Slint original)
        let surface = if dark { rgb(0x2c2d2e) } else { rgb(0xffffff) };
        let sep_line = if dark { rgba(0xffffff0d) } else { rgba(0x00000010) };
        let btn_hover = if dark { rgb(0x2b2c2d) } else { rgb(0xf0f1f8) };
        let _accent = if dark { rgb(0x7ecba3) } else { rgb(0x6ab890) };
        let text_1 = if dark { rgb(0xeaebec) } else { rgb(0x1a1c2e) };
        let text_2 = if dark { rgb(0x919496) } else { rgb(0x7c809a) };
        let _danger = rgb(0xff5f57);
        let fav_color = rgb(0xd8a155);

        div()
            .w(px(164.))
            .rounded(px(8.))
            .bg(surface)
            .border(px(1.))
            .border_color(rgba(0xffffff14))
            .shadow_lg()
            .p(px(4.))
            .flex()
            .flex_col()
            .children(self.items.iter().map(|item| {
                let on_action = on_action.clone();

                // Render separator
                if item.label == SEPARATOR {
                    return div()
                        .w(px(156.))
                        .flex()
                        .flex_col()
                        .child(div().h(px(3.)))
                        .child(div().w_full().h(px(1.)).bg(sep_line))
                        .child(div().h(px(3.)));
                }

                let _is_danger = item.danger;
                let is_fav = item.fav;
                let action = item.action.clone();
                let icon = item.icon.clone();
                let label = item.label.clone();

                let normal_icon = if is_fav { fav_color } else { text_2 };
                let normal_text = if is_fav { fav_color } else { text_1 };

                div()
                    .w(px(156.))
                    .h(px(30.))
                    .rounded(px(5.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .px(px(8.))
                    .gap(px(8.))
                    .cursor(CursorStyle::PointingHand)
                    .hover(move |style| style.bg(btn_hover))
                    .child({
                        let icon = icon.clone();
                        div()
                            .text_size(px(13.))
                            .text_color(normal_icon)
                            .child(icon)
                    })
                    .child({
                        let label = label.clone();
                        div()
                            .text_size(px(13.))
                            .text_color(normal_text)
                            .child(label)
                    })
                    .on_mouse_down(MouseButton::Left, {
                        let action = action.clone();
                        move |_ev, window, cx| {
                            if let Some(ref handler) = on_action {
                                handler(&action, window, cx);
                            }
                        }
                    })
            }))
    }
}
