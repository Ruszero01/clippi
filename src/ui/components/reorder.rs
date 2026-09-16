//! Shared native GPUI drag preview and insertion targets.

use std::{cell::Cell, rc::Rc};

use gpui::{prelude::*, *};

use crate::ui::theme::ClippiTheme;

#[derive(Clone, PartialEq, Eq)]
pub enum ReorderKey {
    Tag(i64),
    Filter(String),
}

impl ReorderKey {
    fn accepts(&self, source: &Self) -> bool {
        self != source && std::mem::discriminant(self) == std::mem::discriminant(source)
    }
}

#[derive(Clone)]
pub struct ReorderDrag {
    pub key: ReorderKey,
    source_bounds: Rc<Cell<Bounds<Pixels>>>,
    render: Rc<dyn Fn() -> AnyElement>,
}

impl ReorderDrag {
    pub fn new(key: ReorderKey, render: impl Fn() -> AnyElement + 'static) -> Self {
        Self {
            key,
            source_bounds: Rc::new(Cell::new(Bounds::default())),
            render: Rc::new(render),
        }
    }
}

struct DragPreview {
    render: Rc<dyn Fn() -> AnyElement>,
    size: Size<Pixels>,
    offset: Point<Pixels>,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Reuse the source's visual element at its measured size. A translucent
        // copy leaves the insertion marker visible, without a separate badge.
        div()
            .relative()
            .left(self.offset.x)
            .top(self.offset.y)
            .w(self.size.width)
            .h(self.size.height)
            .opacity(0.65)
            .child((self.render)())
    }
}

/// Track the whole row, including when only a nested handle starts the drag.
pub fn track_drag_bounds(row: Stateful<Div>, drag: &ReorderDrag) -> Stateful<Div> {
    let bounds = drag.source_bounds.clone();
    row.relative().child(
        canvas(move |layout, _, _| bounds.set(layout), |_, _, _, _| {})
            .absolute()
            .size_full()
            .top_0()
            .left_0(),
    )
}

fn bind_drag(row: Stateful<Div>, drag: ReorderDrag) -> Stateful<Div> {
    row.on_drag(drag, |drag, cursor_offset, window, cx| {
        let bounds = drag.source_bounds.get();
        let offset = preview_offset(bounds.origin, window.mouse_position(), cursor_offset);
        cx.new(|_| DragPreview {
            render: drag.render.clone(),
            size: bounds.size,
            offset,
        })
    })
}

fn preview_offset(
    origin: Point<Pixels>,
    mouse: Point<Pixels>,
    cursor_offset: Point<Pixels>,
) -> Point<Pixels> {
    // GPUI places the drag view at mouse - cursor_offset. Compensate for a
    // handle's position within the row so the grabbed point never jumps.
    origin - mouse + cursor_offset
}

pub fn drag_source(row: Stateful<Div>, drag: ReorderDrag) -> Stateful<Div> {
    bind_drag(track_drag_bounds(row, &drag), drag)
}

/// The same passive six-dot grip is used by the row and its drag preview.
pub fn drag_grip(theme: &ClippiTheme) -> Div {
    let color = theme.text_3;
    div()
        .w(px(22.))
        .h(px(22.))
        .flex_shrink_0()
        .rounded(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .children((0..3).map(|_| {
                    div()
                        .flex()
                        .gap(px(2.))
                        .children((0..2).map(|_| div().size(px(2.)).rounded_full().bg(color)))
                })),
        )
}

pub fn drag_handle(drag: ReorderDrag, theme: &ClippiTheme) -> Stateful<Div> {
    let hover = theme.btn_hover;
    bind_drag(
        drag_grip(theme)
            .id("reorder-handle")
            .cursor(CursorStyle::OpenHand)
            .hover(move |style| style.bg(hover))
            // Keep the full row available as a drop target while preventing a toggle.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation()),
        drag,
    )
}

/// Grid cells insert left/right; list rows insert above/below. Bounds are
/// recorded during prepaint so dropping uses the current scrolled position.
pub fn drop_target(
    row: Stateful<Div>,
    key: ReorderKey,
    horizontal: bool,
    theme: &ClippiTheme,
    on_drop: impl Fn(&ReorderKey, bool, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
    let highlighted = Rc::new(Cell::new(false));
    let accent = theme.accent;
    let hover = theme.accent_overlay();
    row.relative()
        .can_drop({
            let key = key.clone();
            move |value, _, _| {
                value
                    .downcast_ref::<ReorderDrag>()
                    .is_some_and(|drag| key.accepts(&drag.key))
            }
        })
        .drag_over::<ReorderDrag>({
            let highlighted = highlighted.clone();
            move |style, drag, _, _| {
                if key.accepts(&drag.key) {
                    highlighted.set(true);
                    style.bg(hover)
                } else {
                    style
                }
            }
        })
        .child(
            canvas(
                {
                    let bounds = bounds.clone();
                    move |layout_bounds, _, _| bounds.set(layout_bounds)
                },
                move |bounds, _, window, cx| {
                    if highlighted.get()
                        && cx.has_active_drag()
                        && bounds.contains(&window.mouse_position())
                    {
                        let after = is_after(bounds, window.mouse_position(), horizontal);
                        let marker = if horizontal {
                            Bounds::new(
                                point(
                                    if after {
                                        bounds.right() - px(2.)
                                    } else {
                                        bounds.left()
                                    },
                                    bounds.top(),
                                ),
                                size(px(2.), bounds.size.height),
                            )
                        } else {
                            Bounds::new(
                                point(
                                    bounds.left(),
                                    if after {
                                        bounds.bottom() - px(2.)
                                    } else {
                                        bounds.top()
                                    },
                                ),
                                size(bounds.size.width, px(2.)),
                            )
                        };
                        window.paint_quad(fill(marker, accent));
                    }
                },
            )
            .absolute()
            .size_full()
            .top_0()
            .left_0(),
        )
        .on_drop(move |drag: &ReorderDrag, window, cx| {
            cx.stop_propagation();
            on_drop(
                &drag.key,
                is_after(bounds.get(), window.mouse_position(), horizontal),
                window,
                cx,
            );
        })
}

fn is_after(bounds: Bounds<Pixels>, position: Point<Pixels>, horizontal: bool) -> bool {
    if horizontal {
        position.x > bounds.center().x
    } else {
        position.y > bounds.center().y
    }
}

#[cfg(test)]
mod tests {
    use super::{is_after, preview_offset, ReorderKey};
    use gpui::{point, px, size, Bounds};

    #[test]
    fn preview_keeps_the_grabbed_point_for_rows_and_nested_handles() {
        let origin = point(px(40.), px(100.));
        let mouse = point(px(248.), px(113.));
        let movement = point(px(-20.), px(32.));
        for cursor_offset in [mouse - origin, point(px(8.), px(10.))] {
            let correction = preview_offset(origin, mouse, cursor_offset);
            assert_eq!(mouse - cursor_offset + correction, origin);
            assert_eq!(
                mouse + movement - cursor_offset + correction,
                origin + movement
            );
        }
    }

    #[test]
    fn insertion_uses_the_current_cell_bounds_and_layout_axis() {
        let bounds = Bounds::new(point(px(50.), px(120.)), size(px(140.), px(30.)));
        assert!(!is_after(bounds, point(px(60.), px(149.)), true));
        assert!(is_after(bounds, point(px(180.), px(121.)), true));
        assert!(!is_after(bounds, point(px(180.), px(121.)), false));
        assert!(is_after(bounds, point(px(60.), px(149.)), false));
    }

    #[test]
    fn drops_reject_self_and_other_list_types() {
        assert!(ReorderKey::Tag(1).accepts(&ReorderKey::Tag(2)));
        assert!(!ReorderKey::Tag(1).accepts(&ReorderKey::Tag(1)));
        assert!(!ReorderKey::Tag(1).accepts(&ReorderKey::Filter("text".into())));
        assert!(!ReorderKey::Filter("text".into()).accepts(&ReorderKey::Tag(1)));
    }
}
