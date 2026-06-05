//! Hover toolbar — appears on card hover, top-right corner.
//!
//! Matches the original Slint ClipboardList.slint hover toolbar:
//! - 22px height pill, 6px border-radius
//! - Semi-transparent background, 1px border
//! - 18x18 iconfont buttons with 2px spacing
//! - Conditional buttons based on content type and selection count

use std::rc::Rc;

use gpui::*;

use crate::core::types::{ContentType, RichData};

use super::theme::ClippiTheme;

/// Properties that determine which toolbar buttons to show.
pub struct HoverToolbarProps {
    pub content_type: ContentType,
    pub has_qr_code: bool,
    pub is_favorite: bool,
    pub selected_count: usize,
    pub is_selected: bool,
}

impl HoverToolbarProps {
    /// Derive from a ClipboardItem with external selection context.
    pub fn from_item(
        item: &crate::core::types::ClipboardItem,
        selected_count: usize,
        is_selected: bool,
    ) -> Self {
        Self {
            content_type: item.content_type,
            has_qr_code: {
                let rich = RichData::from_json(&item.rich_data);
                rich.qr_text.is_some()
            },
            is_favorite: item.is_favorite,
            selected_count,
            is_selected,
        }
    }
}

#[derive(IntoElement)]
pub struct HoverToolbar {
    props: HoverToolbarProps,
    theme: ClippiTheme,
    on_action: Option<Rc<dyn Fn(&str, &mut Window, &mut App)>>,
}

impl HoverToolbar {
    pub fn new(props: HoverToolbarProps) -> Self {
        Self {
            props,
            theme: ClippiTheme::dark(),
            on_action: None,
        }
    }

    pub fn theme(mut self, theme: ClippiTheme) -> Self {
        self.theme = theme;
        self
    }

    pub fn on_action(mut self, handler: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_action = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for HoverToolbar {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Self {
            props,
            theme,
            on_action,
        } = self;

        // Theme colors (dark mode — matching Slint original)
        let accent = theme.accent;
        let text_2 = theme.text_2;
        let danger = theme.danger;
        let fav_color = theme.fav_color;
        let is_dark = theme.bg == rgb(0x191a1b);
        let pill_bg = if is_dark {
            rgba(0x232425e8)
        } else {
            rgba(0xfffffff0)
        };
        let pill_border = if is_dark {
            rgba(0xffffff20)
        } else {
            rgba(0x00000014)
        };
        let hover_bg = if is_dark {
            rgba(0xffffff10)
        } else {
            rgba(0x0000000a)
        };

        let is_single = props.selected_count <= 1;
        let is_batch = props.selected_count > 1 && props.is_selected;

        // Build button list: (icon_glyph, action_name, hover_color)
        // Using type alias for the color function
        type ColorFn = Box<dyn Fn(bool) -> Rgba>;
        let mut buttons: Vec<(&str, &str, ColorFn)> = Vec::new();

        if is_single {
            // Copy
            buttons.push((
                "\u{e600}",
                "copy",
                Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
            ));

            // Open image (image only)
            if props.content_type == ContentType::Image {
                buttons.push((
                    "\u{e626}",
                    "open_image",
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }

            // QR code (image with QR code)
            if props.content_type == ContentType::Image && props.has_qr_code {
                buttons.push((
                    "\u{e605}",
                    "qr_action",
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }

            // Open location (link / path / file)
            if props.content_type == ContentType::Link
                || props.content_type == ContentType::Path
                || props.content_type == ContentType::File
            {
                buttons.push((
                    "\u{e6d7}",
                    "open_location",
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }

            // Edit (not image or file)
            if props.content_type != ContentType::Image && props.content_type != ContentType::File {
                buttons.push((
                    "\u{e648}",
                    "edit",
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }

            // Note
            buttons.push((
                "\u{e606}",
                "edit_note",
                Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
            ));

            // Favorite (icon changes based on state)
            let fav_icon = if props.is_favorite {
                "\u{e630}"
            } else {
                "\u{e68d}"
            };
            buttons.push((
                fav_icon,
                "toggle_favorite",
                Box::new(move |_hovered: bool| fav_color),
            ));

            // Delete
            buttons.push((
                "\u{e8b6}",
                "delete",
                Box::new(move |hovered: bool| if hovered { danger } else { text_2 }),
            ));
        } else if is_batch {
            // Batch paste
            buttons.push((
                "\u{e600}",
                "batch_paste",
                Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
            ));
            // Batch favorite
            buttons.push((
                "\u{e630}",
                "batch_favorite",
                Box::new(move |_hovered: bool| fav_color),
            ));
            // Batch delete
            buttons.push((
                "\u{e8b6}",
                "batch_delete",
                Box::new(move |hovered: bool| if hovered { danger } else { text_2 }),
            ));
        }

        if buttons.is_empty() {
            return div();
        }

        // Compute width: N buttons x 18px + (N-1) x 2px spacing + 10px padding
        let n = buttons.len();
        let content_w = (n * 18 + (n.saturating_sub(1)) * 2) as f32;
        let toolbar_w = content_w + 10.0;

        div()
            .h(px(22.))
            .w(px(toolbar_w))
            .rounded(px(6.))
            .bg(pill_bg)
            .border(px(1.))
            .border_color(pill_border)
            .flex()
            .flex_row()
            .px(px(5.))
            .items_center()
            .gap(px(2.))
            .children(buttons.into_iter().map(move |(icon, action, color_fn)| {
                let on_action = on_action.clone();
                let action = action.to_string();
                let icon = icon.to_string();

                div()
                    .w(px(18.))
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .cursor(CursorStyle::PointingHand)
                    .hover(move |style| style.bg(hover_bg))
                    .child({
                        let icon = icon.clone();
                        let color_normal = color_fn(false);
                        let color_hover = color_fn(true);
                        div()
                            .font_family("iconfont")
                            .text_size(px(12.))
                            .text_color(color_normal)
                            .hover(move |style| style.text_color(color_hover))
                            .child(icon)
                    })
                    .on_mouse_down(MouseButton::Left, {
                        let action = action.clone();
                        move |_ev, _window, cx| {
                            if let Some(ref handler) = on_action {
                                handler(&action, _window, cx);
                            }
                        }
                    })
            }))
    }
}
