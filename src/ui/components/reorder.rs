//! Shared drag reordering: native drag previews, insertion targets, vertical
//! row geometry, and edge auto-scrolling for scrollable lists.

use std::{cell::Cell, rc::Rc};

use gpui::{prelude::*, *};

use crate::ui::theme::ClippiTheme;

/// Distance from a scroll viewport edge at which a drag starts auto-scrolling.
const AUTOSCROLL_ZONE: f32 = 24.;
/// Auto-scroll speed bounds, in pixels per frame, at the zone edge and the wall.
const AUTOSCROLL_MIN_SPEED: f32 = 1.5;
const AUTOSCROLL_MAX_SPEED: f32 = 12.;

#[derive(Clone, PartialEq, Eq)]
pub enum ReorderKey {
    Tag(i64),
}

impl ReorderKey {
    fn accepts(&self, source: &Self) -> bool {
        // A list only accepts its own kind of row, and never the dragged row.
        matches!((self, source), (Self::Tag(target), Self::Tag(source)) if target != source)
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

/// Grid cells insert left/right; list rows insert above/below. Bounds are
/// recorded during prepaint so dropping uses the current scrolled position.
pub fn drop_target(
    row: Stateful<Div>,
    key: ReorderKey,
    horizontal: bool,
    theme: &ClippiTheme,
    on_drop: impl Fn(ReorderKey, bool, &mut Window, &mut App) + 'static,
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
                drag.key.clone(),
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

/// Vertical drag geometry for row lists: the row top follows the pointer while
/// staying inside the list, and the row sitting under that top is the target.
pub fn row_drag_position(pointer_y: f32, grab_y: f32, count: usize, pitch: f32) -> (f32, usize) {
    let last = count.saturating_sub(1);
    let top = (pointer_y - grab_y).clamp(0., last as f32 * pitch);
    let slot = ((top / pitch).round() as usize).min(last);
    (top, slot)
}

/// Scroll delta for one frame of edge auto-scrolling; negative scrolls down.
/// Zero unless the pointer is over the viewport and within an edge zone.
pub fn auto_scroll_step(viewport: Bounds<Pixels>, mouse: Point<Pixels>) -> f32 {
    let left = f32::from(viewport.left());
    let top = f32::from(viewport.top());
    let width = f32::from(viewport.size.width);
    let height = f32::from(viewport.size.height);
    let x = f32::from(mouse.x);
    if x < left || x > left + width {
        return 0.;
    }
    // A short viewport halves the zones so they never overlap.
    let zone = (height / 2.).min(AUTOSCROLL_ZONE);
    if zone <= 0. {
        return 0.;
    }
    let y = f32::from(mouse.y);
    if y < top - zone || y > top + height + zone {
        return 0.;
    }
    if y < top + zone {
        auto_scroll_speed((top + zone - y) / zone)
    } else if y > top + height - zone {
        -auto_scroll_speed((y - (top + height - zone)) / zone)
    } else {
        0.
    }
}

fn auto_scroll_speed(depth: f32) -> f32 {
    let depth = depth.clamp(0., 1.);
    AUTOSCROLL_MIN_SPEED + depth * (AUTOSCROLL_MAX_SPEED - AUTOSCROLL_MIN_SPEED)
}

/// Symmetric easing shared by the row shuffle and drop settle animations.
pub fn ease_in_out(delta: f32) -> f32 {
    if delta < 0.5 {
        2.0 * delta * delta
    } else {
        1.0 - (-2.0 * delta + 2.0).powi(2) / 2.0
    }
}

/// Cursor of a drag handle at rest: an open hand where the platform has one.
/// Windows has no hand cursors, so GPUI renders `OpenHand` as the default arrow
/// there and the pointer hand is the only grabbable-looking option.
#[cfg(not(target_os = "windows"))]
pub const GRAB_CURSOR: CursorStyle = CursorStyle::OpenHand;
#[cfg(target_os = "windows")]
pub const GRAB_CURSOR: CursorStyle = CursorStyle::PointingHand;

/// Cursor of a handle that is holding a row.
#[cfg(not(target_os = "windows"))]
pub const GRABBING_CURSOR: CursorStyle = CursorStyle::ClosedHand;
#[cfg(target_os = "windows")]
pub const GRABBING_CURSOR: CursorStyle = CursorStyle::PointingHand;

/// Owns a reorder list's scroll offset so a drag can scroll it without the wheel.
/// The scrollable child must track [`Self::handle`], and [`Self::viewport_hook`]
/// must be placed in the same viewport element as an absolute overlay.
#[derive(Clone)]
pub struct DragAutoScroll {
    handle: ScrollHandle,
    viewport: Rc<Cell<Bounds<Pixels>>>,
}

impl Default for DragAutoScroll {
    fn default() -> Self {
        Self::new()
    }
}

impl DragAutoScroll {
    pub fn new() -> Self {
        Self {
            handle: ScrollHandle::new(),
            viewport: Rc::new(Cell::new(Bounds::default())),
        }
    }

    /// Scroll offset source: pass to `track_scroll` and to `scrollbar`.
    pub fn handle(&self) -> &ScrollHandle {
        &self.handle
    }

    /// Return to the top, e.g. when the hosting panel is opened again.
    pub fn reset(&self) {
        self.handle.set_offset(Point::default());
    }

    /// Absolute overlay recording the viewport and scrolling it during a drag.
    pub fn viewport_hook(&self) -> impl IntoElement {
        let track = self.clone();
        let scroll = self.clone();
        canvas(
            move |bounds, _, _| track.viewport.set(bounds),
            move |_, _, window, cx| scroll.scroll_frame(window, cx),
        )
        .absolute()
        .size_full()
        .top_0()
        .left_0()
    }

    fn scroll_frame(&self, window: &mut Window, cx: &App) {
        if !cx.has_active_drag() {
            return;
        }
        let step = auto_scroll_step(self.viewport.get(), window.mouse_position());
        if step == 0. {
            return;
        }
        let offset = self.handle.offset();
        let limit = f32::from(self.handle.max_offset().height);
        let target = (f32::from(offset.y) + step).clamp(-limit, 0.);
        // Standing still against an end of the list stops the frame loop.
        if px(target) == offset.y {
            return;
        }
        self.handle.set_offset(point(offset.x, px(target)));
        window.request_animation_frame();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        auto_scroll_step, is_after, preview_offset, row_drag_position, ReorderKey,
        AUTOSCROLL_MAX_SPEED, AUTOSCROLL_MIN_SPEED,
    };
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
    fn drops_reject_the_dragged_row_itself() {
        assert!(ReorderKey::Tag(1).accepts(&ReorderKey::Tag(2)));
        assert!(!ReorderKey::Tag(1).accepts(&ReorderKey::Tag(1)));
    }

    #[test]
    fn vertical_drag_keeps_the_grab_offset_and_stays_inside_the_list() {
        assert_eq!(row_drag_position(-100., 8., 4, 30.), (0., 0));
        assert_eq!(row_drag_position(1000., 8., 4, 30.), (90., 3));
        assert_eq!(row_drag_position(43., 8., 4, 30.), (35., 1));
        assert_eq!(row_drag_position(500., 8., 1, 30.), (0., 0));
        assert_eq!(row_drag_position(500., 8., 0, 30.), (0., 0));
    }

    #[test]
    fn auto_scroll_only_runs_inside_the_viewport_edge_zones() {
        let viewport = Bounds::new(point(px(10.), px(100.)), size(px(200.), px(200.)));
        let at = |x: f32, y: f32| auto_scroll_step(viewport, point(px(x), px(y)));

        assert_eq!(at(100., 200.), 0.);
        assert_eq!(at(5., 110.), 0.);
        assert_eq!(at(100., 60.), 0.);
        assert_eq!(at(100., 400.), 0.);
        // Reaching the walls scrolls fastest.
        assert_eq!(at(100., 100.), AUTOSCROLL_MAX_SPEED);
        assert_eq!(at(100., 300.), -AUTOSCROLL_MAX_SPEED);
        // Inside a zone the speed ramps up towards the edge.
        let mild = at(100., 120.);
        assert!(mild > AUTOSCROLL_MIN_SPEED && mild < AUTOSCROLL_MAX_SPEED);
        let strong = at(100., 112.);
        assert!(strong > mild);
        // Dragging past an edge keeps scrolling at full speed.
        assert_eq!(at(100., 80.), AUTOSCROLL_MAX_SPEED);
        assert_eq!(at(100., 320.), -AUTOSCROLL_MAX_SPEED);
    }

    #[test]
    fn auto_scroll_is_disabled_for_viewports_without_room() {
        let flat = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(0.)));
        assert_eq!(auto_scroll_step(flat, point(px(100.), px(0.))), 0.);
    }
}
