//! Tag picker panel — assign or remove tags on clipboard items.

use std::rc::Rc;

use gpui::*;

use crate::core::types::TagInfo;

#[derive(IntoElement)]
pub struct TagPickerPanel {
    tags: Vec<(TagInfo, TagState)>,
    on_toggle: Option<Rc<dyn Fn(i64, &mut Window, &mut App)>>,
    on_add: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TagState {
    None,
    All,
    Partial,
}

impl TagPickerPanel {
    pub fn new(tags: Vec<TagInfo>, assigned_ids: &[i64], is_batch: bool) -> Self {
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

        div()
            .flex()
            .flex_col()
            .min_w(px(160.))
            .bg(rgb(0x25262a))
            .border_color(rgb(0x3d3e42))
            .border(px(1.))
            .rounded(px(8.))
            .shadow_md()
            .py(px(4.))
            .child(
                div()
                    .px(px(10.))
                    .py(px(4.))
                    .text_size(px(10.))
                    .text_color(rgb(0x888888))
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
                    .text_color(if is_active {
                        rgb(0xe0e0e0)
                    } else {
                        rgb(0x999999)
                    })
                    .cursor(CursorStyle::PointingHand)
                    .hover(|style| style.bg(rgba(0xffffff10)));

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
                            rgb(0x3d7ef5)
                        } else {
                            rgb(0x3d3e42)
                        })
                        .text_size(px(10.))
                        .text_color(rgb(0xffffff))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(check),
                )
                .child(tag.name.clone())
            }))
            .child(div().h(px(1.)).bg(rgb(0x3d3e42)).mx(px(8.)))
            .child({
                let on_add = on_add;
                let row = div()
                    .px(px(10.))
                    .py(px(6.))
                    .text_size(px(12.))
                    .text_color(rgb(0x888888))
                    .cursor(CursorStyle::PointingHand)
                    .hover(|style| style.bg(rgba(0xffffff10)))
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
