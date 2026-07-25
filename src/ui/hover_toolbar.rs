//! Hover toolbar — appears on card hover, top-right corner.
//!
//! --- Matches the original Slint ClipboardList.slint hover toolbar: ---
//! --- - 22px height pill, 6px border-radius ---
//! --- - Semi-transparent background, 1px border ---
//! --- - 18x18 iconfont buttons with 2px spacing ---
//! --- - Conditional buttons based on content type and selection count ---

use std::rc::Rc;

type ToolbarActionHandler = Rc<dyn Fn(&str, &mut gpui::Window, &mut gpui::App)>;

use gpui::*;
use gpui_component::tooltip::Tooltip;

use crate::core::i18n_keys::I18nKey;
use crate::core::types::{ContentType, RichData};

use super::theme::ClippiTheme;

/// Properties that determine which toolbar buttons to show.
pub struct HoverToolbarProps {
    pub content_type: ContentType,
    pub meta_type: String,
    pub full_text: String,
    pub has_rich_content: bool,
    pub has_qr_code: bool,
    pub is_favorite: bool,
    pub selected_count: usize,
    pub is_selected: bool,
    pub can_merge_selection: bool,
    pub is_transfer: bool,
    pub transfer_is_local: bool,
}

impl HoverToolbarProps {
    /// Derive from a ClipboardItem with external selection context.
    pub fn from_item(
        item: &crate::core::types::ClipboardItem,
        selected_count: usize,
        is_selected: bool,
    ) -> Self {
        Self {
            is_transfer: item.id < 0 && item.meta_type == "transfer",
            transfer_is_local: crate::core::types::FileData::from_json(&item.file_data)
                .files
                .first()
                .is_some_and(|file| !file.path.is_empty()),
            content_type: item.content_type,
            meta_type: item.meta_type.clone(),
            full_text: item.full_text.clone(),
            has_rich_content: matches!(
                item.display_kind(),
                crate::core::types::DisplayKind::Html
                    | crate::core::types::DisplayKind::Markdown
                    | crate::core::types::DisplayKind::Rtf
            ),
            has_qr_code: {
                let rich = RichData::from_json(&item.rich_data);
                rich.qr_text.is_some()
            },
            is_favorite: item.is_favorite,
            selected_count,
            is_selected,
            can_merge_selection: false,
        }
    }

    pub fn can_merge_selection(mut self, value: bool) -> Self {
        self.can_merge_selection = value;
        self
    }
}

#[derive(IntoElement)]
pub struct HoverToolbar {
    props: HoverToolbarProps,
    theme: ClippiTheme,
    on_action: Option<ToolbarActionHandler>,
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

        // --- Build button list: (icon_glyph, action_name, tooltip, hover_color) ---
        // Using type alias for the color function
        type ColorFn = Box<dyn Fn(bool) -> Rgba>;
        let mut buttons: Vec<(&str, &str, SharedString, ColorFn)> = Vec::new();

        if is_single && props.is_transfer {
            if props.transfer_is_local {
                buttons.push((
                    "\u{e609}",
                    "open_transfer_location",
                    I18nKey::CtxOpenFolder.text().into(),
                    Box::new(move |hovered| if hovered { accent } else { text_2 }),
                ));
            } else {
                buttons.push((
                    "\u{e7c8}",
                    "download_transfer",
                    I18nKey::DownloadTransfer.text().into(),
                    Box::new(move |hovered| if hovered { accent } else { text_2 }),
                ));
            }
            buttons.push((
                "\u{e8b6}",
                "delete_transfer",
                I18nKey::RemoveFromTransfer.text().into(),
                Box::new(move |hovered| if hovered { danger } else { text_2 }),
            ));
        } else if is_single {
            // Copy
            buttons.push((
                "\u{e7e1}",
                "copy",
                I18nKey::CtxCopy.text().into(),
                Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
            ));

            // --- Paste Plain Text (only when item has actual rich formatting) ---
            if props.has_rich_content {
                buttons.push((
                    "\u{e60e}",
                    "paste_plain",
                    I18nKey::CtxPastePlain.text().into(),
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }

            // --- Open image (image only) ---
            if props.content_type == ContentType::Image {
                buttons.push((
                    "\u{e626}",
                    "open_image",
                    I18nKey::CtxOpenImage.text().into(),
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }

            // --- QR code (image with QR code) ---
            if props.content_type == ContentType::Image && props.has_qr_code {
                buttons.push((
                    "\u{e605}",
                    "qr_action",
                    I18nKey::CtxDetectQr.text().into(),
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }

            // --- Open in browser (link only) ---
            if props.meta_type == "link" {
                buttons.push((
                    "\u{e643}",
                    "open_location",
                    I18nKey::CtxOpenLink.text().into(),
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }
            // --- Jump to directory (path only, native platform only) ---
            if props.meta_type == "path"
                && crate::core::types::path_is_native(&props.full_text)
                && crate::core::types::path_exists(&props.full_text)
            {
                buttons.push((
                    "\u{e609}",
                    "open_location",
                    I18nKey::CtxOpenFolder.text().into(),
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }
            // --- Reveal in folder (file only) ---
            if props.content_type == ContentType::File {
                buttons.push((
                    "\u{e609}",
                    "open_location",
                    I18nKey::CtxOpenFolder.text().into(),
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }

            // --- Edit (not image or file) ---
            if props.content_type != ContentType::Image && props.content_type != ContentType::File {
                buttons.push((
                    "\u{e648}",
                    "edit",
                    I18nKey::CtxEdit.text().into(),
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }

            // Note
            buttons.push((
                "\u{e606}",
                "edit_note",
                I18nKey::EditNote.text().into(),
                Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
            ));

            // Tag
            buttons.push((
                "\u{ec07}",
                "show_tag_picker",
                I18nKey::CtxTag.text().into(),
                Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
            ));

            // --- Favorite (icon changes based on state) ---
            let fav_icon = if props.is_favorite {
                "\u{e630}"
            } else {
                "\u{e68d}"
            };
            let fav_tooltip: SharedString = if props.is_favorite {
                I18nKey::CtxUnfav.text().into()
            } else {
                I18nKey::CtxFav.text().into()
            };
            buttons.push((
                fav_icon,
                "toggle_favorite",
                fav_tooltip,
                Box::new(move |_hovered: bool| fav_color),
            ));

            // --- Delete ---
            buttons.push((
                "\u{e8b6}",
                "delete",
                I18nKey::CtxDelete.text().into(),
                Box::new(move |hovered: bool| if hovered { danger } else { text_2 }),
            ));
        } else if is_batch && props.is_transfer {
            buttons.push((
                "\u{e8b6}",
                "batch_delete_transfer",
                I18nKey::RemoveSelectedFromTransfer.text().into(),
                Box::new(move |hovered: bool| if hovered { danger } else { text_2 }),
            ));
        } else if is_batch {
            // --- Batch paste ---
            let batch_paste_tip: SharedString =
                I18nKey::CtxBatchPasteN.fmt(&[&props.selected_count.to_string()]).into();
            buttons.push((
                "\u{e63f}",
                "batch_paste",
                batch_paste_tip,
                Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
            ));
            if props.can_merge_selection {
                // --- Merge selected ---
                buttons.push((
                    "\u{e68a}",
                    "merge_selected",
                    I18nKey::CtxMergeSelected.text().into(),
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }
            // --- Batch tag ---
            buttons.push((
                "\u{ec07}",
                "batch_tag",
                I18nKey::CtxBatchTag.text().into(),
                Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
            ));
            // --- Batch favorite ---
            buttons.push((
                "\u{e630}",
                "batch_favorite",
                I18nKey::CtxBatchFav.text().into(),
                Box::new(move |_hovered: bool| fav_color),
            ));
            // --- Batch delete ---
            buttons.push((
                "\u{e8b6}",
                "batch_delete",
                I18nKey::CtxBatchDelete.text().into(),
                Box::new(move |hovered: bool| if hovered { danger } else { text_2 }),
            ));
        }

        if buttons.is_empty() {
            return div();
        }

        // --- Compute width: N buttons x 18px + (N-1) x 2px spacing + 10px padding ---
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
            .children(buttons.into_iter().map(move |(icon, action, tooltip, color_fn)| {
                let on_action = on_action.clone();
                let action = action.to_string();
                let icon = icon.to_string();
                let tooltip_text = tooltip.clone();
                let button_id = SharedString::from(format!("hover-toolbar-{action}"));

                div()
                    .id(button_id)
                    .w(px(18.))
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .cursor(CursorStyle::PointingHand)
                    .hover(move |style| style.bg(hover_bg))
                    .tooltip(move |window, cx| {
                        let tooltip_text = tooltip_text.clone();
                        Tooltip::element(move |_window, _cx| {
                            div().text_size(px(10.)).child(tooltip_text.clone())
                        })
                        .build(window, cx)
                    })
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
                            cx.stop_propagation();
                            if let Some(ref handler) = on_action {
                                handler(&action, _window, cx);
                            }
                        }
                    })
            }))
    }
}
