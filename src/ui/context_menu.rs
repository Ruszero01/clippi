//! Context menu — right-click menu for clipboard items.
//!
//! Matches the original Slint ContextMenu.slint design:
//! - 164px width, 8px border-radius, 4px padding
//! - 30px item height, 5px border-radius per item
//! - Icon (13px iconfont) + label (13px) per row
//! - Separators: 3px gap + 1px line + 3px gap
//! - Single and batch mode variants with conditional items
//! - Position clamping to container bounds

use std::rc::Rc;

use gpui::*;

/// Context describing which menu items to show.
#[derive(Default)]
pub struct MenuItemContext {
    pub is_image: bool,
    pub is_file: bool,
    pub is_color: bool,
    /// true = current format is HEX → show "Paste as RGB"
    pub is_hex: bool,
    pub is_favorite: bool,
}

impl MenuItemContext {
    pub fn from_item(item: &crate::core::types::ClipboardItem) -> Self {
        use crate::core::types::ContentType;
        let is_color = item.content_type == ContentType::Color;
        // is_hex = true → show "Paste as RGB" (convert FROM hex)
        let is_hex = if is_color {
            item.full_text.trim_start().to_lowercase().starts_with('#')
        } else {
            false
        };
        Self {
            is_image: item.content_type == ContentType::Image,
            is_file: item.content_type == ContentType::File,
            is_color,
            is_hex,
            is_favorite: item.is_favorite,
        }
    }
}

#[derive(IntoElement)]
pub struct ContextMenu {
    items: Vec<RawMenuItem>,
    x: f32,
    y: f32,
    container_width: f32,
    container_height: f32,
    on_action: Option<Rc<dyn Fn(&str, &mut Window, &mut App)>>,
    on_dismiss: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}

/// Internal menu item descriptor.
struct RawMenuItem {
    label: String,
    action: String,
    icon: String,
    danger: bool,
    fav: bool,
}

const SEPARATOR_LABEL: &str = "__sep__";
const MENU_WIDTH: f32 = 164.0;
/// Height of a single menu item row.
const ITEM_HEIGHT: f32 = 30.0;
/// Height of a separator (3px gap + 1px line + 3px gap).
const SEPARATOR_HEIGHT: f32 = 7.0;
/// Vertical padding (4px top + 4px bottom).
const MENU_V_PADDING: f32 = 8.0;

impl ContextMenu {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            x: 0.0,
            y: 0.0,
            container_width: f32::MAX,
            container_height: f32::MAX,
            on_action: None,
            on_dismiss: None,
        }
    }

    /// Build a single-item context menu from item context.
    pub fn for_item(ctx: &MenuItemContext) -> Self {
        let mut items: Vec<RawMenuItem> = Vec::new();

        // Copy
        items.push(RawMenuItem {
            label: "Copy".into(),
            action: "copy".into(),
            icon: "\u{e600}".into(),
            danger: false,
            fav: false,
        });
        // Paste
        items.push(RawMenuItem {
            label: "Paste".into(),
            action: "paste".into(),
            icon: "\u{e600}".into(),
            danger: false,
            fav: false,
        });

        // Color conversion (only for color type)
        if ctx.is_color {
            let (label, action) = if ctx.is_hex {
                ("Paste as RGB", "paste_as_rgb")
            } else {
                ("Paste as HEX", "paste_as_hex")
            };
            items.push(RawMenuItem {
                label: label.into(),
                action: action.into(),
                icon: "\u{e610}".into(),
                danger: false,
                fav: false,
            });
        }

        // Separator
        items.push(RawMenuItem {
            label: SEPARATOR_LABEL.into(),
            action: String::new(),
            icon: String::new(),
            danger: false,
            fav: false,
        });

        // Edit (not for image/file)
        if !ctx.is_image && !ctx.is_file {
            items.push(RawMenuItem {
                label: "Edit".into(),
                action: "edit".into(),
                icon: "\u{e648}".into(),
                danger: false,
                fav: false,
            });
        }

        // Note
        items.push(RawMenuItem {
            label: "Note".into(),
            action: "edit_note".into(),
            icon: "\u{e606}".into(),
            danger: false,
            fav: false,
        });

        // Open original image (image only)
        if ctx.is_image {
            items.push(RawMenuItem {
                label: "Open image".into(),
                action: "open_image".into(),
                icon: "\u{e626}".into(),
                danger: false,
                fav: false,
            });
            items.push(RawMenuItem {
                label: "Paste OCR text".into(),
                action: "paste_ocr".into(),
                icon: "\u{e648}".into(),
                danger: false,
                fav: false,
            });
            items.push(RawMenuItem {
                label: "Detect QR Code".into(),
                action: "qr_detect".into(),
                icon: "\u{e605}".into(),
                danger: false,
                fav: false,
            });
        }

        // Tag
        items.push(RawMenuItem {
            label: "Tag".into(),
            action: "show_tag_picker".into(),
            icon: "\u{ec07}".into(),
            danger: false,
            fav: false,
        });

        // Separator
        items.push(RawMenuItem {
            label: SEPARATOR_LABEL.into(),
            action: String::new(),
            icon: String::new(),
            danger: false,
            fav: false,
        });

        // Favorite
        let (fav_label, fav_icon) = if ctx.is_favorite {
            ("Unfav", "\u{e630}")
        } else {
            ("Fav", "\u{e68d}")
        };
        items.push(RawMenuItem {
            label: fav_label.into(),
            action: "toggle_favorite".into(),
            icon: fav_icon.into(),
            danger: false,
            fav: true,
        });

        // Delete
        items.push(RawMenuItem {
            label: "Delete".into(),
            action: "delete".into(),
            icon: "\u{e8b6}".into(),
            danger: true,
            fav: false,
        });

        Self::new().items(items)
    }

    /// Build a batch context menu.
    pub fn for_batch(selected_count: usize) -> Self {
        let items = vec![
            RawMenuItem {
                label: format!("Paste {} items", selected_count),
                action: "batch_paste".into(),
                icon: "\u{e600}".into(),
                danger: false,
                fav: false,
            },
            RawMenuItem {
                label: SEPARATOR_LABEL.into(),
                action: String::new(),
                icon: String::new(),
                danger: false,
                fav: false,
            },
            RawMenuItem {
                label: "Batch tag".into(),
                action: "show_tag_picker".into(),
                icon: "\u{ec07}".into(),
                danger: false,
                fav: false,
            },
            RawMenuItem {
                label: SEPARATOR_LABEL.into(),
                action: String::new(),
                icon: String::new(),
                danger: false,
                fav: false,
            },
            RawMenuItem {
                label: "Batch fav".into(),
                action: "batch_favorite".into(),
                icon: "\u{e630}".into(),
                danger: false,
                fav: true,
            },
            RawMenuItem {
                label: "Batch delete".into(),
                action: "batch_delete".into(),
                icon: "\u{e8b6}".into(),
                danger: true,
                fav: false,
            },
        ];
        Self::new().items(items)
    }

    fn items(mut self, items: Vec<RawMenuItem>) -> Self {
        self.items = items;
        self
    }

    pub fn with_position(
        mut self,
        x: f32,
        y: f32,
        container_width: f32,
        container_height: f32,
    ) -> Self {
        self.x = x;
        self.y = y;
        self.container_width = container_width;
        self.container_height = container_height;
        self
    }

    pub fn on_action(mut self, handler: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_action = Some(Rc::new(handler));
        self
    }

    pub fn on_dismiss(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }

    /// Estimate the rendered height of this menu based on its items.
    pub fn estimated_height(&self) -> f32 {
        let mut h: f32 = MENU_V_PADDING;
        for item in &self.items {
            if item.label == SEPARATOR_LABEL {
                h += SEPARATOR_HEIGHT;
            } else {
                h += ITEM_HEIGHT;
            }
        }
        h
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        // Compute menu height BEFORE destructuring self
        let menu_h = self.estimated_height();

        let Self {
            items,
            x,
            y,
            container_width,
            container_height,
            on_action,
            on_dismiss,
        } = self;

        // Dark theme colors (matching Slint original)
        let surface = rgb(0x2c2d2e);
        let sep_line = rgba(0xffffff0d);
        let btn_hover = rgb(0x2b2c2d);
        let accent = rgb(0x7ecba3);
        let text_1 = rgb(0xeaebec);
        let text_2 = rgb(0x919496);
        let danger = rgb(0xff5f57);
        let fav_color = rgb(0xd8a155);

        // Clamp position to container bounds with height awareness.
        // Flips the menu above the cursor if it would overflow the bottom edge.
        let menu_w = MENU_WIDTH;
        let clamped_x = x.clamp(4.0, (container_width - menu_w - 4.0).max(4.0));
        // Prefer below cursor; flip above if it overflows the bottom.
        let clamped_y = if y + menu_h + 4.0 <= container_height {
            // Fits below cursor — small 2px gap from click point
            y.clamp(4.0, container_height - menu_h - 4.0)
        } else {
            // Flip above cursor — 8px gap from click point
            (y - menu_h - 8.0).clamp(4.0, container_height - menu_h - 4.0)
        };

        div()
            .absolute()
            .left(px(clamped_x))
            .top(px(clamped_y))
            .w(px(menu_w))
            .rounded(px(8.))
            .bg(surface)
            .border(px(1.))
            .border_color(rgba(0xffffff14))
            .shadow_lg()
            .p(px(4.))
            .flex()
            .flex_col()
            .children(items.into_iter().map(|item| {
                let on_action = on_action.clone();
                let on_dismiss = on_dismiss.clone();

                // Render separator
                if item.label == SEPARATOR_LABEL {
                    return div()
                        .w(px(156.))
                        .flex()
                        .flex_col()
                        .child(div().h(px(3.)))
                        .child(div().w_full().h(px(1.)).bg(sep_line))
                        .child(div().h(px(3.)));
                }

                let is_danger = item.danger;
                let is_fav = item.fav;
                let action = item.action.clone();
                let icon = item.icon.clone();
                let label = item.label.clone();

                let normal_icon = if is_fav {
                    fav_color
                } else if is_danger {
                    danger
                } else {
                    text_2
                };
                let normal_text = if is_fav {
                    fav_color
                } else if is_danger {
                    danger
                } else {
                    text_1
                };

                // Hover colors: fav stays fav, danger stays danger, otherwise accent
                let hover_icon = if is_fav {
                    fav_color
                } else if is_danger {
                    danger
                } else {
                    accent
                };
                let hover_text = if is_fav {
                    fav_color
                } else if is_danger {
                    danger
                } else {
                    accent
                };

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
                            .font_family("iconfont")
                            .text_size(px(13.))
                            .text_color(normal_icon)
                            .hover(move |style| style.text_color(hover_icon))
                            .child(icon)
                    })
                    .child({
                        let label = label.clone();
                        div()
                            .text_size(px(13.))
                            .text_color(normal_text)
                            .hover(move |style| style.text_color(hover_text))
                            .child(label)
                    })
                    .on_mouse_down(MouseButton::Left, {
                        let action = action.clone();
                        move |_ev, window, cx| {
                            if let Some(ref handler) = on_action {
                                handler(&action, window, cx);
                            }
                            if let Some(ref dismiss) = on_dismiss {
                                dismiss(window, cx);
                            }
                        }
                    })
            }))
    }
}
