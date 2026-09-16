//! Sidebar - tag navigation sidebar matching Slint `SideTagBar.slint`.
//!
//! --- Slint behavior: ---
//! --- - width 56px, rows placed every 24px; ---
//! --- - checked tags slide to x=0 and are fully opaque; ---
//! --- - unchecked pinned tags remain visible with dimmed text; ---
//! --- - unchecked, unpinned tags slide slightly right and fade out; ---
//! --- - left click toggles a visible tag filter, right click toggles pin. ---

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_transitions::WindowUseTransition;

use crate::core::types::TagInfo;
use crate::state::app::AppState;

use super::clipboard_list::ClipboardListView;
use super::components::reorder::{ease_in_out, row_drag_position};
use super::theme::ClippiTheme;

/// Sidebar top offset (must match `top(px(65.))` in root.rs).
/// Aligned with type filter bar top: titlebar(30) + pt(1) + search(28) + mb(6) = 65.
const SIDEBAR_TOP_OFFSET: f32 = 65.0;
/// Height of a single sidebar row (22px row + 2px gap).
const ROW_HEIGHT: f32 = 24.0;
/// Bottom padding to keep the last row from touching the window edge.
const SIDEBAR_BOTTOM_PADDING: f32 = 8.0;

/// A reorder preview is local to the sidebar until the button is released.
struct SidebarDrag {
    id: i64,
    ids: Vec<i64>,
    start: Point<Pixels>,
    grab_y: f32,
    top: f32,
    target: usize,
    moved: bool,
}

fn drag_position(pointer_y: f32, grab_y: f32, count: usize) -> (f32, usize) {
    row_drag_position(pointer_y, grab_y, count, ROW_HEIGHT)
}

/// Sidebar entity for displaying and managing tags.
pub struct Sidebar {
    state: Entity<AppState>,
    list_view: Entity<ClipboardListView>,
    rendered_tag_ids: Vec<i64>,
    unchecked_unpinned_since: HashMap<i64, Instant>,
    transition_generations: HashMap<i64, u64>,
    dark_mode: bool,
    drag: Option<SidebarDrag>,
    settling: Option<(i64, f32, Instant)>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl Sidebar {
    pub fn new(
        state: Entity<AppState>,
        list_view: Entity<ClipboardListView>,
        theme: &ClippiTheme,
    ) -> Self {
        let dark_mode = theme.bg == rgb(0x191a1b);
        Self {
            state,
            list_view,
            rendered_tag_ids: Vec::new(),
            unchecked_unpinned_since: HashMap::new(),
            transition_generations: HashMap::new(),
            dark_mode,
            drag: None,
            settling: None,
            bounds: Rc::new(Cell::new(Bounds::default())),
        }
    }

    pub fn cancel_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let drag = self.drag.take();
        let cancelled = drag.is_some();
        if let Some(drag) = drag.filter(|drag| drag.moved) {
            self.settling = Some((drag.id, drag.top, Instant::now()));
        }
        if cancelled {
            cx.notify();
        }
        cancelled
    }

    fn move_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        if !drag.moved && (position - drag.start).magnitude() <= 4. {
            return;
        }
        drag.moved = true;
        (drag.top, drag.target) = drag_position(
            f32::from(position.y - self.bounds.get().top()),
            drag.grab_y,
            drag.ids.len(),
        );
        cx.notify();
    }

    fn finish_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        // Include the final pointer position even when no last move event arrived.
        self.move_drag(position, cx);
        let Some(drag) = self.drag.take() else {
            return;
        };
        if drag.moved {
            self.settling = Some((drag.id, drag.top, Instant::now()));
            if let Some(source) = drag.ids.iter().position(|id| *id == drag.id) {
                self.state.update(cx, |state, cx| {
                    state.reorder_tag(drag.id, drag.ids[drag.target], source < drag.target);
                    cx.notify();
                });
            }
        } else if self.bounds.get().contains(&position) {
            let items = self.state.update(cx, |state, _| {
                state.toggle_tag_filter(drag.id);
                state.visible_items()
            });
            self.list_view
                .update(cx, |list, cx| list.set_items(items, cx));
        }
        cx.notify();
    }

    /// Update theme (called when user changes theme in settings).
    pub fn set_theme(&mut self, theme: &ClippiTheme, cx: &mut Context<Self>) {
        self.dark_mode = theme.bg == rgb(0x191a1b);
        cx.notify();
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (tags, active_tag_ids, pinned_tag_ids) = {
            let app_state = self.state.read(cx);
            (
                app_state.tags.clone(),
                app_state.filters.tag_ids.clone(),
                app_state.settings.pinned_tag_ids.clone(),
            )
        };

        let now = Instant::now();
        let active_or_pinned_ids: Vec<i64> = tags
            .iter()
            .filter(|tag| active_tag_ids.contains(&tag.id) || pinned_tag_ids.contains(&tag.id))
            .map(|tag| tag.id)
            .collect();

        let previous_rendered_ids = self.rendered_tag_ids.clone();
        for previous_id in previous_rendered_ids.clone() {
            if !active_or_pinned_ids.contains(&previous_id) {
                self.unchecked_unpinned_since
                    .entry(previous_id)
                    .or_insert(now);
            }
        }
        self.unchecked_unpinned_since.retain(|id, started_at| {
            !active_or_pinned_ids.contains(id)
                && now.duration_since(*started_at) < Duration::from_millis(300)
        });

        let mut display_tags = ordered_sidebar_tags(
            &tags,
            &active_tag_ids,
            &pinned_tag_ids,
            &self.unchecked_unpinned_since,
        );
        for tag in &display_tags {
            let newly_entering = active_tag_ids.contains(&tag.id)
                && !pinned_tag_ids.contains(&tag.id)
                && !previous_rendered_ids.contains(&tag.id);
            if newly_entering {
                *self.transition_generations.entry(tag.id).or_insert(0) += 1;
            }
        }
        let transition_generations = self.transition_generations.clone();
        self.rendered_tag_ids = display_tags.iter().map(|tag| tag.id).collect();

        // ── Collapse tags that don't fit in the available vertical space ──
        let available_height =
            f32::from(window.viewport_size().height) - SIDEBAR_TOP_OFFSET - SIDEBAR_BOTTOM_PADDING;
        let max_visible = (available_height / ROW_HEIGHT).floor() as usize;

        let hidden_count = display_tags.len().saturating_sub(max_visible);
        if hidden_count > 0 {
            // Keep last slot for the "+N" overflow indicator row.
            let overflow_start = max_visible.saturating_sub(1);
            let overflow_tags: Vec<_> = display_tags.drain(overflow_start..).collect();
            // Clean up animation state for tags that are now invisible.
            for tag in &overflow_tags {
                self.rendered_tag_ids.retain(|id| *id != tag.id);
                self.unchecked_unpinned_since.remove(&tag.id);
                self.transition_generations.remove(&tag.id);
            }
        }
        // ── end collapse ──

        let visible_ids: Vec<_> = display_tags.iter().map(|tag| tag.id).collect();
        if self
            .drag
            .as_ref()
            .is_some_and(|drag| drag.ids != visible_ids)
        {
            self.cancel_drag(cx);
        }
        let mut preview_ids = visible_ids.clone();
        let dragging_id = self
            .drag
            .as_ref()
            .filter(|drag| drag.moved)
            .map(|drag| drag.id);
        let dragged_top = self.drag.as_ref().map(|drag| drag.top).unwrap_or(0.);
        let settling = self.settling;
        if self
            .settling
            .is_some_and(|(_, _, start)| start.elapsed() >= Duration::from_millis(140))
        {
            self.settling = None;
        }
        if let Some(drag) = &self.drag {
            if let Some(source) = preview_ids.iter().position(|id| *id == drag.id) {
                preview_ids.remove(source);
                preview_ids.insert(drag.target, drag.id);
            }
        }
        // Paint the dragged row last, inside the clipped sidebar, above its siblings.
        display_tags.sort_by_key(|tag| Some(tag.id) == dragging_id);
        let rows_height = visible_ids.len() as f32 * ROW_HEIGHT;
        let sidebar_bounds = self.bounds.clone();
        let sidebar_for_move = cx.entity();
        let sidebar_for_up = cx.entity();
        let sidebar_for_press = cx.entity();
        let dark = self.dark_mode;
        let text_1 = if dark { rgb(0xeaebec) } else { rgb(0x1a1c2e) };
        let row_bg_default = if dark { rgb(0x2a2b2e) } else { rgb(0xf5f6fa) };
        let row_bg_hover = if dark { rgb(0x353638) } else { rgb(0xeceef4) };
        let state_for_click = self.state.clone();
        let sidebar_for_notify = cx.entity().clone();
        let duration = Duration::from_millis(250);

        div()
            .relative()
            .w(px(56.))
            .h(px(
                rows_height + if hidden_count > 0 { ROW_HEIGHT } else { 0. }
            ))
            .overflow_hidden()
            .bg(rgba(0x00000000))
            .child(
                canvas(
                    move |bounds, _, _| sidebar_bounds.set(bounds),
                    move |_, _, window, _| {
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture
                                && sidebar_for_move.read(cx).drag.is_some()
                            {
                                sidebar_for_move.update(cx, |sidebar, cx| {
                                    if event.pressed_button == Some(MouseButton::Left) {
                                        sidebar.move_drag(event.position, cx);
                                    } else {
                                        sidebar.cancel_drag(cx);
                                    }
                                });
                                cx.stop_propagation();
                            }
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture
                                && event.button == MouseButton::Left
                                && sidebar_for_up.read(cx).drag.is_some()
                            {
                                sidebar_for_up.update(cx, |sidebar, cx| {
                                    sidebar.finish_drag(event.position, cx)
                                });
                                cx.stop_propagation();
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(display_tags.into_iter().map(move |tag| {
                let checked = active_tag_ids.contains(&tag.id);
                let pinned = pinned_tag_ids.contains(&tag.id);
                let interactable = checked || pinned;
                let entering = checked && !pinned && !previous_rendered_ids.contains(&tag.id);
                let target_row_x = if checked { px(0.0) } else { px(22.0) };
                let target_opacity = if interactable { 1.0 } else { 0.0 };
                let target_text_opacity = if checked { 1.0 } else { 0.3 };
                let target_bar_height = if pinned { px(16.0) } else { px(6.0) };
                let initial_row_x = if entering { px(22.0) } else { target_row_x };
                let initial_opacity = if entering { 0.0 } else { target_opacity };
                let initial_text_opacity = if entering { 0.3 } else { target_text_opacity };
                let initial_bar_height = if entering { px(6.0) } else { target_bar_height };
                let bar_color = parse_tag_color(&tag.color);
                let label = tag.name;
                let tag_id = tag.id;
                let transition_generation =
                    transition_generations.get(&tag_id).copied().unwrap_or(0);
                let tag_key = (tag_id as u64).wrapping_add(transition_generation << 32);
                let state_for_right = state_for_click.clone();
                let sidebar_for_right = sidebar_for_notify.clone();

                let row_x_transition = window
                    .use_keyed_transition(("sidebar-tag-x", tag_key), cx, duration, move |_, _| {
                        initial_row_x
                    })
                    .with_easing(ease_in_out);
                row_x_transition.update(cx, |value, cx| {
                    *value = target_row_x;
                    cx.notify();
                });
                let row_x = *row_x_transition.evaluate(window, cx);

                let opacity_transition = window
                    .use_keyed_transition(
                        ("sidebar-tag-opacity", tag_key),
                        cx,
                        duration,
                        move |_, _| initial_opacity,
                    )
                    .with_easing(ease_in_out);
                opacity_transition.update(cx, |value, cx| {
                    *value = target_opacity;
                    cx.notify();
                });
                let opacity = *opacity_transition.evaluate(window, cx);

                let text_opacity_transition = window
                    .use_keyed_transition(
                        ("sidebar-tag-text-opacity", tag_key),
                        cx,
                        duration,
                        move |_, _| initial_text_opacity,
                    )
                    .with_easing(ease_in_out);
                text_opacity_transition.update(cx, |value, cx| {
                    *value = target_text_opacity;
                    cx.notify();
                });
                let text_opacity = *text_opacity_transition.evaluate(window, cx);

                let bar_height_transition = window
                    .use_keyed_transition(
                        ("sidebar-tag-bar-height", tag_key),
                        cx,
                        duration,
                        move |_, _| initial_bar_height,
                    )
                    .with_easing(ease_in_out);
                bar_height_transition.update(cx, |value, cx| {
                    *value = target_bar_height;
                    cx.notify();
                });
                let bar_height = *bar_height_transition.evaluate(window, cx);

                let original_slot = visible_ids.iter().position(|id| *id == tag_id).unwrap_or(0);
                let target_slot = preview_ids
                    .iter()
                    .position(|id| *id == tag_id)
                    .unwrap_or(original_slot);
                let y_transition = window
                    .use_keyed_transition(
                        ("sidebar-tag-y", tag_id as u64),
                        cx,
                        Duration::from_millis(140),
                        move |_, _| px(original_slot as f32 * ROW_HEIGHT),
                    )
                    .with_easing(ease_in_out);
                y_transition.update(cx, |value, cx| {
                    let target = px(target_slot as f32 * ROW_HEIGHT);
                    if *value != target {
                        *value = target;
                        cx.notify();
                    }
                });
                let animated_y = *y_transition.evaluate(window, cx);
                let row_y = if Some(tag_id) == dragging_id {
                    px(dragged_top)
                } else if let Some((_, top, started)) = settling.filter(|(id, _, _)| *id == tag_id)
                {
                    let delta = (started.elapsed().as_secs_f32() / 0.14).min(1.);
                    if delta < 1. {
                        window.request_animation_frame();
                    }
                    px(top + (target_slot as f32 * ROW_HEIGHT - top) * ease_in_out(delta))
                } else {
                    animated_y
                };

                let row_visual = std::rc::Rc::new(move || {
                    div()
                        .w(px(56.))
                        .h(px(22.))
                        .opacity(opacity)
                        .rounded(px(4.))
                        .bg(row_bg_default)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(5.))
                        .child(div().w(px(3.)).h(bar_height).rounded(px(2.)).bg(bar_color))
                        .child(
                            div()
                                .w(px(43.))
                                .h(px(22.))
                                .flex()
                                .items_center()
                                .text_size(px(11.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_1)
                                .opacity(text_opacity)
                                .overflow_hidden()
                                .child(label.clone()),
                        )
                });

                let row = row_visual()
                    .id(("sidebar-tag", tag_id as u64))
                    .absolute()
                    .left(row_x)
                    .top(row_y)
                    .when(Some(tag_id) == dragging_id, |row| row.bg(row_bg_hover))
                    .cursor(if interactable {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::Arrow
                    })
                    .when(interactable, |row| {
                        row.hover(move |style| style.bg(row_bg_hover))
                    });

                if interactable {
                    let sidebar = sidebar_for_press.clone();
                    let ids = visible_ids.clone();
                    row.on_mouse_down(MouseButton::Left, move |event, _, cx| {
                        cx.stop_propagation();
                        sidebar.update(cx, |sidebar, cx| {
                            sidebar.settling = None;
                            sidebar.drag = Some(SidebarDrag {
                                id: tag_id,
                                ids: ids.clone(),
                                start: event.position,
                                grab_y: f32::from(
                                    event.position.y - sidebar.bounds.get().top() - row_y,
                                ),
                                top: f32::from(row_y),
                                target: original_slot,
                                moved: false,
                            });
                            cx.notify();
                        });
                    })
                    .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                        state_for_right.update(cx, |state, _| state.toggle_pinned_tag(tag_id));
                        sidebar_for_right.update(cx, |_, cx| cx.notify());
                    })
                } else {
                    row
                }
            }))
            .when(hidden_count > 0, move |parent| {
                let pill_bg = if dark {
                    rgba(0x232425e8)
                } else {
                    rgba(0xffffffe8)
                };
                let pill_border = if dark {
                    rgba(0xffffff20)
                } else {
                    rgba(0x00000014)
                };
                let text_2 = if dark { rgb(0x919496) } else { rgb(0x7c809a) };
                // 38px = 36px visible + 2px overlap to prevent gap with main panel edge
                parent.child(
                    div()
                        .absolute()
                        .top(px(rows_height))
                        .w(px(38.))
                        .flex()
                        .justify_end()
                        .child(
                            div()
                                .h(px(18.))
                                .rounded_l(px(9.))
                                .bg(pill_bg)
                                .border(px(1.))
                                .border_color(pill_border)
                                .px(px(5.))
                                .flex()
                                .items_center()
                                .text_size(px(9.))
                                .text_color(text_2)
                                .cursor(CursorStyle::PointingHand)
                                .child(format!("+{hidden_count}")),
                        ),
                )
            })
    }
}

/// Parse a hex color string like "#EF4444" into an Rgba.
fn parse_tag_color(hex: &str) -> Rgba {
    let s = hex.trim_start_matches('#');
    if s.len() == 6 {
        let r = u32::from_str_radix(&s[0..2], 16).unwrap_or(0x7e);
        let g = u32::from_str_radix(&s[2..4], 16).unwrap_or(0xcb);
        let b = u32::from_str_radix(&s[4..6], 16).unwrap_or(0xa3);
        rgba((r << 24) | (g << 16) | (b << 8) | 0xff)
    } else {
        rgb(0x7ecba3)
    }
}

fn ordered_sidebar_tags(
    tags: &[TagInfo],
    active_tag_ids: &[i64],
    pinned_tag_ids: &[i64],
    unchecked_unpinned_since: &HashMap<i64, Instant>,
) -> Vec<TagInfo> {
    // Pinning controls visibility, while the shared tag order controls position.
    tags.iter()
        .filter(|tag| {
            pinned_tag_ids.contains(&tag.id)
                || active_tag_ids.contains(&tag.id)
                || unchecked_unpinned_since.contains_key(&tag.id)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{drag_position, ordered_sidebar_tags};
    use crate::core::types::TagInfo;
    use std::{collections::HashMap, time::Instant};

    #[test]
    fn sidebar_drag_clamps_to_rows_and_respects_the_grab_offset() {
        assert_eq!(drag_position(-100., 8., 4), (0., 0));
        assert_eq!(drag_position(1000., 8., 4), (72., 3));
        assert_eq!(drag_position(43., 8., 4), (35., 1));
        assert_eq!(drag_position(45., 8., 4), (37., 2));
        assert_eq!(drag_position(500., 8., 1), (0., 0));
        assert_eq!(drag_position(500., 8., 0), (0., 0));
    }

    #[test]
    fn sidebar_keeps_shared_order_for_pinned_active_and_fading_tags() {
        let tags: Vec<_> = [4, 2, 1, 3, 5]
            .into_iter()
            .map(|id| TagInfo {
                id,
                uid: id.to_string(),
                name: id.to_string(),
                color: "FF0000".into(),
                updated_at: String::new(),
            })
            .collect();
        let fading = HashMap::from([(1, Instant::now())]);
        let ordered = ordered_sidebar_tags(&tags, &[2, 4], &[3, 4], &fading);
        assert_eq!(
            ordered.iter().map(|tag| tag.id).collect::<Vec<_>>(),
            [4, 2, 1, 3]
        );
    }
}
