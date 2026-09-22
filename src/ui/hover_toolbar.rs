//! Hover toolbar — appears on card hover, top-right corner.
//!
//! --- Matches the original Slint ClipboardList.slint hover toolbar: ---
//! --- - 22px height pill, 6px border-radius ---
//! --- - Semi-transparent background, 1px border ---
//! --- - 18x18 iconfont buttons with 2px spacing ---
//! --- - Conditional buttons based on content type and selection count ---

use std::rc::Rc;

use crate::ui::font::sc;

type ToolbarActionHandler = Rc<dyn Fn(&str, &mut gpui::Window, &mut gpui::App)>;

use gpui::prelude::*;
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
    /// True when the transfer entry is pinned (remote manifest state).
    pub transfer_pinned: bool,
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
            transfer_pinned: item
                .tags
                .iter()
                .any(|tag| tag.uid == crate::core::transfer_types::TRANSFER_STATUS_PINNED_UID),
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

/// Which open action a single entry offers, if any.
///
/// Links open in the browser, paths and files reveal in the file manager, and
/// text (including markdown/HTML/color entries) opens in the system's default
/// text editor.
fn single_open_action(props: &HoverToolbarProps) -> Option<&'static str> {
    if props.meta_type == "link" {
        return Some("open_location");
    }
    if props.meta_type == "path" {
        // A path that does not exist on this machine has nothing to reveal.
        return (crate::core::types::path_is_native(&props.full_text)
            && crate::core::types::path_exists(&props.full_text))
        .then_some("open_location");
    }
    match props.content_type {
        ContentType::File => Some("open_location"),
        ContentType::Image => Some("open_image"),
        ContentType::PlainText | ContentType::RichText => {
            // Secrets are masked in the UI on purpose — never spill one into a
            // plain-text file on disk.
            (props.meta_type != "secret").then_some("open_text")
        }
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

        // --- Build button list: (icon_glyph, action_name, hover_color) ---
        // Using type alias for the color function
        type ColorFn = Box<dyn Fn(bool) -> Rgba>;
        let mut buttons: Vec<(&str, &str, ColorFn)> = Vec::new();

        if is_single && props.is_transfer {
            let transfer_pinned = props.transfer_pinned;
            let pin_color = theme.transfer_pin_color;
            if props.transfer_is_local {
                buttons.push((
                    "\u{e603}",
                    "open_transfer_location",
                    Box::new(move |hovered| if hovered { accent } else { text_2 }),
                ));
            } else {
                buttons.push((
                    "\u{e7c8}",
                    "download_transfer",
                    Box::new(move |hovered| if hovered { accent } else { text_2 }),
                ));
            }
            // Pin/unpin — between open/download and delete.
            buttons.push((
                "\u{e633}",
                "toggle_transfer_pin",
                Box::new(move |hovered| {
                    if hovered || transfer_pinned {
                        pin_color
                    } else {
                        text_2
                    }
                }),
            ));
            buttons.push((
                "\u{e696}",
                "delete_transfer",
                Box::new(move |hovered| if hovered { danger } else { text_2 }),
            ));
        } else if is_single {
            // Copy
            buttons.push((
                "\u{e7e1}",
                "copy",
                Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
            ));

            // --- Paste Plain Text (only when item has actual rich formatting) ---
            if props.has_rich_content {
                buttons.push((
                    "\u{e606}",
                    "paste_plain",
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }

            // --- Paste as bitmap (image only) ---
            if props.content_type == ContentType::Image {
                buttons.push((
                    "\u{e626}",
                    "paste_image_bitmap",
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }

            // --- QR code (image with QR code) ---
            if props.content_type == ContentType::Image && props.has_qr_code {
                buttons.push((
                    "\u{e605}",
                    "qr_action",
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }

            // --- Edit (not image or file) ---
            if props.content_type != ContentType::Image && props.content_type != ContentType::File {
                buttons.push((
                    "\u{e679}",
                    "edit",
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }

            // Note
            buttons.push((
                "\u{e6fa}",
                "edit_note",
                Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
            ));

            // Tag
            buttons.push((
                "\u{e886}",
                "show_tag_picker",
                Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
            ));

            // --- Open (one button for the whole family, right of tag) ---
            if let Some(open_action) = single_open_action(&props) {
                buttons.push((
                    "\u{e603}",
                    open_action,
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }

            // --- Favorite (icon changes based on state) ---
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

            // --- Delete ---
            buttons.push((
                "\u{e696}",
                "delete",
                Box::new(move |hovered: bool| if hovered { danger } else { text_2 }),
            ));
        } else if is_batch && props.is_transfer {
            buttons.push((
                "\u{e696}",
                "batch_delete_transfer",
                Box::new(move |hovered: bool| if hovered { danger } else { text_2 }),
            ));
        } else if is_batch {
            // --- Batch paste ---
            buttons.push((
                "\u{e63f}",
                "batch_paste",
                Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
            ));
            if props.can_merge_selection {
                // --- Merge selected ---
                buttons.push((
                    "\u{e68a}",
                    "merge_selected",
                    Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
                ));
            }
            // --- Batch tag ---
            buttons.push((
                "\u{e886}",
                "batch_tag",
                Box::new(move |hovered: bool| if hovered { accent } else { text_2 }),
            ));
            // --- Batch favorite ---
            buttons.push((
                "\u{e630}",
                "batch_favorite",
                Box::new(move |_hovered: bool| fav_color),
            ));
            // --- Batch delete ---
            buttons.push((
                "\u{e696}",
                "batch_delete",
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
        let is_favorite = props.is_favorite;
        let selected_count = props.selected_count;
        let meta_type = props.meta_type.clone();

        div()
            .h(sc(22.))
            .w(sc(toolbar_w))
            .rounded(sc(6.))
            .bg(pill_bg)
            .border(sc(1.))
            .border_color(pill_border)
            .flex()
            .flex_row()
            .px(sc(5.))
            .items_center()
            .gap(sc(2.))
            .children(buttons.into_iter().map(move |(icon, action, color_fn)| {
                let on_action = on_action.clone();
                let tooltip = match action {
                    "copy" => I18nKey::CtxCopy.text().to_string(),
                    "paste_plain" => I18nKey::CtxPastePlain.text().to_string(),
                    "paste_image_bitmap" => I18nKey::CtxPasteImageBitmap.text().to_string(),
                    "open_image" => I18nKey::CtxOpenImage.text().to_string(),
                    "open_text" => I18nKey::CtxOpenText.text().to_string(),
                    "qr_action" => I18nKey::CtxDetectQr.text().to_string(),
                    "open_location" if meta_type == "link" => {
                        I18nKey::CtxOpenLink.text().to_string()
                    }
                    "open_location" => I18nKey::CtxOpenFolder.text().to_string(),
                    "edit" => I18nKey::CtxEdit.text().to_string(),
                    "edit_note" => I18nKey::EditNote.text().to_string(),
                    "show_tag_picker" => I18nKey::CtxTag.text().to_string(),
                    "toggle_favorite" if is_favorite => I18nKey::CtxUnfav.text().to_string(),
                    "toggle_favorite" => I18nKey::CtxFav.text().to_string(),
                    "delete" => I18nKey::CtxDelete.text().to_string(),
                    "batch_paste" => I18nKey::CtxBatchPasteN.fmt(&[&selected_count.to_string()]),
                    "merge_selected" => I18nKey::CtxMergeSelected.text().to_string(),
                    "batch_tag" => I18nKey::CtxBatchTag.text().to_string(),
                    "batch_favorite" => I18nKey::CtxBatchFav.text().to_string(),
                    "batch_delete" => I18nKey::CtxBatchDelete.text().to_string(),
                    "open_transfer_location" => I18nKey::CtxOpenFolder.text().to_string(),
                    "download_transfer" => I18nKey::DownloadTransfer.text().to_string(),
                    "toggle_transfer_pin" if props.transfer_pinned => {
                        I18nKey::TransferUnpin.text().to_string()
                    }
                    "toggle_transfer_pin" => I18nKey::TransferPin.text().to_string(),
                    "delete_transfer" => I18nKey::RemoveFromTransfer.text().to_string(),
                    "batch_delete_transfer" => {
                        I18nKey::RemoveSelectedFromTransfer.text().to_string()
                    }
                    _ => action.to_string(),
                };
                let action_id = action;
                let action = action.to_string();
                let icon = icon.to_string();

                div()
                    .id(action_id)
                    .w(sc(18.))
                    .h(sc(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(sc(3.))
                    .cursor(CursorStyle::PointingHand)
                    .hover(move |style| style.bg(hover_bg))
                    .tooltip(move |window, cx| {
                        let label = tooltip.clone();
                        Tooltip::element(move |_window, _cx| {
                            div().text_size(sc(10.)).child(label.clone())
                        })
                        .build(window, cx)
                    })
                    .child({
                        let icon = icon.clone();
                        let color_normal = color_fn(false);
                        let color_hover = color_fn(true);
                        div()
                            .font_family("iconfont")
                            .text_size(sc(12.))
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

#[cfg(test)]
mod open_action_tests {
    use super::{single_open_action, HoverToolbarProps};
    use crate::core::types::{ClipboardItem, ContentType, RichData};

    fn props_for(item: &ClipboardItem) -> HoverToolbarProps {
        HoverToolbarProps::from_item(item, 1, false)
    }

    fn text_item(id: i64, meta_type: &str) -> ClipboardItem {
        let mut item = ClipboardItem::new_text(id, "content", ContentType::PlainText, None, None);
        item.meta_type = meta_type.to_string();
        item
    }

    #[test]
    fn each_entry_type_routes_to_its_open_action() {
        let link = text_item(1, "link");
        assert_eq!(single_open_action(&props_for(&link)), Some("open_location"));

        let mut file = text_item(2, "");
        file.content_type = ContentType::File;
        assert_eq!(single_open_action(&props_for(&file)), Some("open_location"));

        let image = ClipboardItem::new_image(3, "", 0, 4, 4, None);
        assert_eq!(single_open_action(&props_for(&image)), Some("open_image"));

        let plain = text_item(4, "");
        assert_eq!(single_open_action(&props_for(&plain)), Some("open_text"));

        // Markdown and HTML are text too — they open as source.
        let markdown = text_item(5, "markdown");
        assert_eq!(single_open_action(&props_for(&markdown)), Some("open_text"));
        let html = ClipboardItem::new_text(
            6,
            "hello",
            ContentType::RichText,
            None,
            Some(&RichData {
                html: Some("<p>hello</p>".into()),
                ..Default::default()
            }),
        );
        assert_eq!(single_open_action(&props_for(&html)), Some("open_text"));
    }

    /// Text that is masked in the UI never gets an open button: it would be
    /// written to a plain-text file on disk.
    #[test]
    fn secrets_are_never_opened_in_an_editor() {
        let secret = text_item(7, "secret");
        assert_eq!(single_open_action(&props_for(&secret)), None);
    }

    /// A path entry that is gone from this machine has nothing to reveal.
    #[test]
    fn a_missing_path_has_no_open_action() {
        let mut missing = ClipboardItem::new_text(
            8,
            &std::env::temp_dir()
                .join("clippi-missing-open-target-9a1f")
                .to_string_lossy(),
            ContentType::PlainText,
            None,
            None,
        );
        missing.meta_type = "path".to_string();
        assert_eq!(single_open_action(&props_for(&missing)), None);

        let mut existing = ClipboardItem::new_text(
            9,
            &std::env::temp_dir().to_string_lossy(),
            ContentType::PlainText,
            None,
            None,
        );
        existing.meta_type = "path".to_string();
        assert_eq!(
            single_open_action(&props_for(&existing)),
            Some("open_location")
        );
    }
}
