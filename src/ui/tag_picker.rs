//! Tag picker panel — assign or remove tags on clipboard items.

use std::rc::Rc;

use gpui::*;

use crate::core::types::TagInfo;

use super::theme::ClippiTheme;

#[derive(IntoElement)]
pub struct TagPickerPanel {
    tags: Vec<(TagInfo, TagState)>,
    on_toggle: Option<Rc<dyn Fn(i64, &mut Window, &mut App)>>,
    on_add: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    theme: ClippiTheme,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TagState {
    None,
    All,
    Partial,
}

impl TagPickerPanel {
    pub fn new(
        tags: Vec<TagInfo>,
        assigned_ids: &[i64],
        is_batch: bool,
        theme: ClippiTheme,
    ) -> Self {
        let tags_with_state = tags
            .into_iter()
            .map(|t| {
                let state = if assigned_ids.contains(&t.id) {
                    if is_batch {
                        TagState::Partial
                    } else {
                        TagState::All
                    }
                } else {
                    TagState::None
                };
                (t, state)
            })
            .collect();
        Self {
            tags: tags_with_state,
            on_toggle: None,
            on_add: None,
            theme,
        }
    }

    pub fn on_toggle(mut self, handler: impl Fn(i64, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }

    pub fn on_add(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_add = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for TagPickerPanel {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let on_toggle = self.on_toggle;
        let on_add = self.on_add;
        let theme = &self.theme;
        let is_dark = theme.bg == rgb(0x191a1b);

        // Panel surface & border
        let surface = theme.panel_surface;
        let border = if is_dark {
            rgba(0xffffff18)
        } else {
            rgba(0x00000012)
        };
        // Separator
        let sep = if is_dark {
            rgba(0xffffff0d)
        } else {
            rgba(0x0000000d)
        };
        // Section header text
        let header_text = theme.text_3;
        // Active/inactive row text
        let row_text_active = theme.text_1;
        let row_text_inactive = theme.text_2;
        // Row hover
        let row_hover = if is_dark {
            rgba(0xffffff10)
        } else {
            rgba(0x00000008)
        };
        // Checkbox
        let checkbox_checked = theme.accent;
        let checkbox_unchecked = if is_dark {
            rgb(0x3d3e42)
        } else {
            rgb(0xd0d2de)
        };
        // Checkbox text
        let checkbox_text = if is_dark {
            rgb(0xffffff)
        } else {
            rgb(0xffffff)
        };
        // Add button text (same as header)
        let add_text = theme.text_2;

        div()
            .flex()
            .flex_col()
            .min_w(px(160.))
            .bg(surface)
            .border_color(border)
            .border(px(1.))
            .rounded(px(8.))
            .shadow_md()
            .py(px(4.))
            .child(
                div()
                    .px(px(10.))
                    .py(px(4.))
                    .text_size(px(10.))
                    .text_color(header_text)
                    .child("ASSIGN TAGS"),
            )
            .children(self.tags.into_iter().map(|(tag, state)| {
                let tag_id = tag.id;
                let on_toggle = on_toggle.clone();
                let check = match state {
                    TagState::All => "✓",
                    TagState::Partial => "-",
                    TagState::None => " ",
                };
                let is_active = state != TagState::None;

                let row = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .py(px(6.))
                    .text_size(px(12.))
                    .text_color(if is_active { row_text_active } else { row_text_inactive })
                    .cursor(CursorStyle::PointingHand)
                    .hover(move |style| style.bg(row_hover));

                let row = if let Some(handler) = on_toggle {
                    row.on_mouse_down(MouseButton::Left, move |_e, w, cx| {
                        handler(tag_id, w, cx);
                    })
                } else {
                    row
                };

                row.child(
                    div()
                        .w(px(14.))
                        .h(px(14.))
                        .rounded(px(3.))
                        .bg(if is_active {
                            checkbox_checked
                        } else {
                            checkbox_unchecked
                        })
                        .text_size(px(10.))
                        .text_color(checkbox_text)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(check),
                )
                .child(tag.name.clone())
            }))
            .child(div().h(px(1.)).bg(sep).mx(px(8.)))
            .child({
                let on_add = on_add;
                let row = div()
                    .px(px(10.))
                    .py(px(6.))
                    .text_size(px(12.))
                    .text_color(add_text)
                    .cursor(CursorStyle::PointingHand)
                    .hover(move |style| style.bg(row_hover))
                    .child("+ New tag");

                if let Some(handler) = on_add {
                    row.on_mouse_down(MouseButton::Left, move |_e, w, cx| {
                        handler(w, cx);
                    })
                } else {
                    row
                }
            })
    }
}
