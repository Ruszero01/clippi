//! Context menu — right-click menu for clipboard items.
//!
//! --- Matches the original Slint ContextMenu.slint design: ---
//! --- - 164px width, 8px border-radius, 4px padding ---
//! --- - 30px item height, 5px border-radius per item ---
//! --- - Icon (13px iconfont) + label (13px) per row ---
//! --- - Separators: 3px gap + 1px line + 3px gap ---
//! --- - Single and batch mode variants with conditional items ---
//! --- - Position clamping to container bounds ---

use std::rc::Rc;

use gpui::*;

use crate::core::i18n_keys::I18nKey;
use crate::ui::theme::ClippiTheme;

type MenuActionHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;
type MenuDismissHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// Context describing which menu items to show.
#[derive(Default)]
pub struct MenuItemContext {
    pub is_image: bool,
    pub is_file: bool,
    pub is_color: bool,
    pub is_rich_text: bool,
    /// true = current format is HEX → show "Paste as RGB"
    pub is_hex: bool,
    pub is_favorite: bool,
    pub is_link: bool,
    pub is_path: bool,
}

impl MenuItemContext {
    pub fn from_item(item: &crate::core::types::ClipboardItem) -> Self {
        use crate::core::types::{ContentType, DisplayKind};
        let is_color = item.meta_type == "color";
        // --- is_hex = true → show "Paste as RGB" (convert FROM hex) ---
        let is_hex = is_color && crate::core::color::is_hex_format(&item.full_text);
        // Only show "paste as plain text" when item has actual rich formatting
        let is_rich_text = matches!(
            item.display_kind(),
            DisplayKind::Html | DisplayKind::Markdown | DisplayKind::Rtf
        );
        Self {
            is_image: item.content_type == ContentType::Image,
            is_file: item.content_type == ContentType::File,
            is_color,
            is_rich_text,
            is_hex,
            is_favorite: item.is_favorite,
            is_link: item.meta_type == "link",
            is_path: item.meta_type == "path",
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
    theme: ClippiTheme,
    on_action: Option<MenuActionHandler>,
    on_dismiss: Option<MenuDismissHandler>,
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
            theme: ClippiTheme::dark(),
            on_action: None,
            on_dismiss: None,
        }
    }

    /// Build a single-item context menu from item context.
    pub fn for_item(ctx: &MenuItemContext) -> Self {
        let mut items: Vec<RawMenuItem> = Vec::new();

        // Copy
        items.push(RawMenuItem {
            label: I18nKey::CtxCopy.text().into(),
            action: "copy".into(),
            icon: "\u{e7e1}".into(),
            danger: false,
            fav: false,
        });
        // --- Paste ---
        items.push(RawMenuItem {
            label: I18nKey::CtxPaste.text().into(),
            action: "paste".into(),
            icon: "\u{e63f}".into(),
            danger: false,
            fav: false,
        });
        // --- Paste Plain Text (rich text only) ---
        if ctx.is_rich_text {
            items.push(RawMenuItem {
                label: I18nKey::CtxPastePlain.text().into(),
                action: "paste_plain".into(),
                icon: "\u{e60e}".into(),
                danger: false,
                fav: false,
            });
        }

        // Color conversion (only for color type)
        if ctx.is_color {
            let (label, action) = if ctx.is_hex {
                (I18nKey::CtxPasteAsRgb.text(), "paste_as_rgb")
            } else {
                (I18nKey::CtxPasteAsHex.text(), "paste_as_hex")
            };
            items.push(RawMenuItem {
                label: label.into(),
                action: action.into(),
                icon: "\u{e610}".into(),
                danger: false,
                fav: false,
            });
        }

        // --- Separator ---
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
                label: I18nKey::CtxEdit.text().into(),
                action: "edit".into(),
                icon: "\u{e648}".into(),
                danger: false,
                fav: false,
            });
        }

        // Note
        items.push(RawMenuItem {
            label: I18nKey::EditNote.text().into(),
            action: "edit_note".into(),
            icon: "\u{e606}".into(),
            danger: false,
            fav: false,
        });

        // --- Open original image (image only) ---
        if ctx.is_image {
            items.push(RawMenuItem {
                label: I18nKey::CtxOpenImage.text().into(),
                action: "open_image".into(),
                icon: "\u{e626}".into(),
                danger: false,
                fav: false,
            });
            items.push(RawMenuItem {
                label: I18nKey::CtxPasteOcr.text().into(),
                action: "paste_ocr".into(),
                icon: "\u{e6a5}".into(),
                danger: false,
                fav: false,
            });
            items.push(RawMenuItem {
                label: I18nKey::CtxDetectQr.text().into(),
                action: "qr_detect".into(),
                icon: "\u{e605}".into(),
                danger: false,
                fav: false,
            });
        }

        // Tag
        items.push(RawMenuItem {
            label: I18nKey::CtxTag.text().into(),
            action: "show_tag_picker".into(),
            icon: "\u{ec07}".into(),
            danger: false,
            fav: false,
        });

        // --- Open in browser (link only) ---
        if ctx.is_link {
            items.push(RawMenuItem {
                label: I18nKey::CtxOpenLink.text().into(),
                action: "open_location".into(),
                icon: "\u{e643}".into(),
                danger: false,
                fav: false,
            });
        }
        // --- Jump to directory (path only) ---
        if ctx.is_path {
            items.push(RawMenuItem {
                label: I18nKey::CtxOpenFolder.text().into(),
                action: "open_location".into(),
                icon: "\u{e609}".into(),
                danger: false,
                fav: false,
            });
        }
        // --- Open folder (file only) ---
        if ctx.is_file {
            items.push(RawMenuItem {
                label: I18nKey::CtxOpenFolder.text().into(),
                action: "open_location".into(),
                icon: "\u{e609}".into(),
                danger: false,
                fav: false,
            });
        }

        // --- Separator ---
        items.push(RawMenuItem {
            label: SEPARATOR_LABEL.into(),
            action: String::new(),
            icon: String::new(),
            danger: false,
            fav: false,
        });

        // --- Favorite ---
        let (fav_label, fav_icon) = if ctx.is_favorite {
            (I18nKey::CtxUnfav.text(), "\u{e630}")
        } else {
            (I18nKey::CtxFav.text(), "\u{e68d}")
        };
        items.push(RawMenuItem {
            label: fav_label.into(),
            action: "toggle_favorite".into(),
            icon: fav_icon.into(),
            danger: false,
            fav: true,
        });

        // --- Delete ---
        items.push(RawMenuItem {
            label: I18nKey::CtxDelete.text().into(),
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
                label: I18nKey::CtxBatchPasteN.fmt(&[&selected_count.to_string()]),
                action: "batch_paste".into(),
                icon: "\u{e63f}".into(),
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
                label: I18nKey::CtxBatchTag.text().into(),
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
                label: I18nKey::CtxBatchFav.text().into(),
                action: "batch_favorite".into(),
                icon: "\u{e630}".into(),
                danger: false,
                fav: true,
            },
            RawMenuItem {
                label: I18nKey::CtxBatchDelete.text().into(),
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

    pub fn theme(mut self, theme: ClippiTheme) -> Self {
        self.theme = theme;
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
        // --- Compute menu height BEFORE destructuring self ---
        let menu_h = self.estimated_height();

        let Self {
            items,
            x,
            y,
            container_width,
            container_height,
            theme,
            on_action,
            on_dismiss,
        } = self;

        let surface = theme.panel_surface;
        let sep_line = theme.panel_sep_line;
        let btn_hover = theme.btn_hover;
        let accent = theme.accent;
        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let danger = theme.danger;
        let fav_color = theme.fav_color;

        // --- Clamp position to container bounds with height awareness. ---
        // Flips the menu above the cursor if it would overflow the bottom edge.
        let menu_w = MENU_WIDTH;
        let clamped_x = x.clamp(4.0, (container_width - menu_w - 4.0).max(4.0));
        // Prefer below cursor; flip above if it overflows the bottom.
        let clamped_y = if y + menu_h + 4.0 <= container_height {
            // --- Fits below cursor — small 2px gap from click point ---
            y.clamp(4.0, container_height - menu_h - 4.0)
        } else {
            // --- Flip above cursor — 8px gap from click point ---
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
            .border_color(sep_line)
            .shadow_lg()
            .p(px(4.))
            .flex()
            .flex_col()
            .on_key_down({
                let on_dismiss = on_dismiss.clone();
                move |ev: &KeyDownEvent, window, cx| {
                    if ev.keystroke.key.as_str() == "escape" {
                        cx.stop_propagation();
                        if let Some(ref dismiss) = on_dismiss {
                            dismiss(window, cx);
                        }
                    }
                }
            })
            .children(items.into_iter().map(|item| {
                let on_action = on_action.clone();
                let on_dismiss = on_dismiss.clone();

                // --- Render separator ---
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

                // --- Hover colors: fav stays fav, danger stays danger, otherwise accent ---
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
