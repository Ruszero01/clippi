//! Type filter config panel — floating panel for configuring which
//! content-type filter buttons are visible and their display order.
//!
//! --- Right-click the filter bar to open. ---
//! --- Each row: circular checkbox (show/hide) + icon + label + drag handle. ---
//! Dragging the handle keeps the row itself under the pointer (no floating
//! copy), clamps it to the list, and slides the remaining rows out of the way.

use crate::ui::font::fs;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::*;
use gpui_transitions::WindowUseTransition;

use crate::core::i18n_keys::I18nKey;
use crate::state::app::AppState;

use super::components::reorder::{
    drag_grip, ease_in_out, row_drag_position, GRABBING_CURSOR, GRAB_CURSOR,
};
use super::filter_bar::{filter_type_display, FilterBar};
use super::theme::ClippiTheme;

/// Row height, row gap, and their sum (the vertical pitch between rows).
const ROW_HEIGHT: f32 = 28.0;
const ROW_PITCH: f32 = 30.0;
/// How long the dragged row takes to settle into its slot after a drop.
const SETTLE_MS: u64 = 140;
/// Pointer travel that turns a press on the handle into a drag.
const DRAG_THRESHOLD: f64 = 4.0;

/// A reorder in progress is local to the panel until the button is released.
struct PanelDrag {
    key: String,
    keys: Vec<String>,
    start: Point<Pixels>,
    grab_y: f32,
    top: f32,
    target: usize,
    moved: bool,
}

pub struct TypeFilterConfigPanel {
    state: Entity<AppState>,
    filter_bar: Entity<FilterBar>,
    drag: Option<PanelDrag>,
    settling: Option<(String, f32, Instant)>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl TypeFilterConfigPanel {
    pub fn new(
        state: Entity<AppState>,
        filter_bar: Entity<FilterBar>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            state,
            filter_bar,
            drag: None,
            settling: None,
            bounds: Rc::new(Cell::new(Bounds::default())),
        }
    }

    pub fn close(&self, cx: &mut App) {
        self.filter_bar
            .update(cx, |bar, cx| bar.close_filter_config(cx));
    }

    fn toggle_visible(&self, key: &str, cx: &mut App) {
        self.state.update(cx, |s, _cx| {
            let mut just_hidden = false;
            if let Some(entry) = s
                .settings
                .type_filter_config
                .iter_mut()
                .find(|e| e.key == key)
            {
                let was_visible = entry.visible;
                entry.visible = !entry.visible;
                just_hidden = was_visible && !entry.visible;
            }
            s.settings.save();
            if just_hidden {
                // The type bar is shared, so a hidden chip must stop filtering
                // in the quick popup as well.
                s.deactivate_type_filter(key);
            } else {
                s.reload_items();
            }
        });
        self.filter_bar.update(cx, |_b, cx| cx.notify());
    }

    /// Drop an in-progress drag without committing it. Returns whether a drag
    /// was active, so callers can treat it as a handled Escape. The row is still
    /// animated back into its slot.
    pub(crate) fn cancel_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let drag = self.drag.take();
        let cancelled = drag.is_some();
        if let Some(drag) = drag.filter(|drag| drag.moved) {
            self.settling = Some((drag.key, drag.top, Instant::now()));
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
        if !drag.moved && (position - drag.start).magnitude() <= DRAG_THRESHOLD {
            return;
        }
        drag.moved = true;
        (drag.top, drag.target) = row_drag_position(
            f32::from(position.y - self.bounds.get().top()),
            drag.grab_y,
            drag.keys.len(),
            ROW_PITCH,
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
            self.settling = Some((drag.key.clone(), drag.top, Instant::now()));
            let source = drag.keys.iter().position(|key| *key == drag.key);
            if let (Some(source), Some(target)) = (source, drag.keys.get(drag.target).cloned()) {
                self.state.update(cx, |state, cx| {
                    state.reorder_type_filter(&drag.key, &target, source < drag.target);
                    cx.notify();
                });
                self.filter_bar.update(cx, |_, cx| cx.notify());
            }
        }
        cx.notify();
    }
}

impl Render for TypeFilterConfigPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (config, theme) = {
            let app_state = self.state.read(cx);
            (
                app_state.settings.type_filter_config.clone(),
                ClippiTheme::from_setting(&app_state.settings.theme, Some(window.appearance())),
            )
        };

        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let surface = theme.panel_surface;
        let sep_line = theme.panel_sep_line;
        let btn_hover = theme.btn_hover;
        let panel_border = if theme.bg == rgb(0x191a1b) {
            rgba(0xffffff14)
        } else {
            rgba(0x00000012)
        };
        let this_entity = cx.entity().clone();

        let keys: Vec<String> = config.iter().map(|entry| entry.key.clone()).collect();
        if self.drag.as_ref().is_some_and(|drag| drag.keys != keys) {
            self.cancel_drag(cx);
        }
        let dragging_key = self
            .drag
            .as_ref()
            .filter(|drag| drag.moved)
            .map(|drag| drag.key.clone());
        let dragged_top = self.drag.as_ref().map(|drag| drag.top).unwrap_or(0.);
        if self
            .settling
            .as_ref()
            .is_some_and(|(_, _, started)| started.elapsed() >= Duration::from_millis(SETTLE_MS))
        {
            self.settling = None;
        }
        let settling = self.settling.clone();

        // Slot of each row once the row under the pointer has taken its place.
        let mut preview_keys = keys.clone();
        if let Some(drag) = &self.drag {
            if let Some(source) = preview_keys.iter().position(|key| *key == drag.key) {
                preview_keys.remove(source);
                preview_keys.insert(drag.target.min(preview_keys.len()), drag.key.clone());
            }
        }

        let rows_height = (keys.len() as f32 * ROW_PITCH - (ROW_PITCH - ROW_HEIGHT)).max(0.);
        let bounds_for_canvas = self.bounds.clone();
        let panel_for_move = cx.entity();
        let panel_for_up = cx.entity();
        let mut rows: Vec<(usize, String, bool)> = config
            .iter()
            .enumerate()
            .map(|(slot, entry)| (slot, entry.key.clone(), entry.visible))
            .collect();
        // Paint the dragged row above the rows sliding out of its way.
        rows.sort_by_key(|(_, key, _)| usize::from(dragging_key.as_deref() == Some(key.as_str())));

        div()
            .flex()
            .flex_col()
            .w(px(240.))
            .bg(surface)
            .border_color(panel_border)
            .border(px(1.))
            .rounded(px(8.))
            .shadow_lg()
            .p(px(8.))
            .gap(px(4.))
            // --- Title row ---
            .child({
                let this = this_entity.clone();
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(24.))
                    .child(
                        div()
                            .text_size(fs(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(I18nKey::FilterConfigTitle.text()),
                    )
                    .child(
                        div()
                            .w(px(22.))
                            .h(px(22.))
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(|el| el.bg(btn_hover))
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                cx.stop_propagation();
                                this.update(cx, |panel, cx| panel.close(cx));
                            })
                            .child(
                                div()
                                    .text_size(fs(14.))
                                    .font_family("iconfont")
                                    .text_color(text_2)
                                    .child("\u{e7b7}"),
                            ),
                    )
            })
            // --- Separator ---
            .child(div().w_full().h(px(1.)).bg(sep_line))
            // --- Item list ---
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(rows_height))
                    .child(
                        canvas(
                            move |bounds, _, _| bounds_for_canvas.set(bounds),
                            move |_, _, window, _| {
                                window.on_mouse_event(
                                    move |event: &MouseMoveEvent, phase, _, cx| {
                                        if phase == DispatchPhase::Capture
                                            && panel_for_move.read(cx).drag.is_some()
                                        {
                                            panel_for_move.update(cx, |panel, cx| {
                                                if event.pressed_button == Some(MouseButton::Left) {
                                                    panel.move_drag(event.position, cx);
                                                } else {
                                                    panel.cancel_drag(cx);
                                                }
                                            });
                                            cx.stop_propagation();
                                        }
                                    },
                                );
                                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                                    if phase == DispatchPhase::Capture
                                        && event.button == MouseButton::Left
                                        && panel_for_up.read(cx).drag.is_some()
                                    {
                                        panel_for_up.update(cx, |panel, cx| {
                                            panel.finish_drag(event.position, cx)
                                        });
                                        cx.stop_propagation();
                                    }
                                });
                            },
                        )
                        .absolute()
                        .size_full()
                        .top_0()
                        .left_0(),
                    )
                    .children(rows.into_iter().map(|(original_slot, key, visible)| {
                        let target_slot = preview_keys
                            .iter()
                            .position(|preview| *preview == key)
                            .unwrap_or(original_slot);
                        let is_dragging = dragging_key.as_deref() == Some(key.as_str());

                        let y_transition = window
                            .use_keyed_transition(
                                SharedString::from(format!("type-filter-y-{key}")),
                                cx,
                                Duration::from_millis(SETTLE_MS),
                                move |_, _| px(original_slot as f32 * ROW_PITCH),
                            )
                            .with_easing(ease_in_out);
                        y_transition.update(cx, |value, cx| {
                            let target = px(target_slot as f32 * ROW_PITCH);
                            if *value != target {
                                *value = target;
                                cx.notify();
                            }
                        });
                        let animated_y = *y_transition.evaluate(window, cx);
                        let row_y = if is_dragging {
                            px(dragged_top)
                        } else if let Some((_, top, started)) = settling
                            .as_ref()
                            .filter(|(settled, _, _)| settled.as_str() == key.as_str())
                        {
                            let delta = (started.elapsed().as_secs_f32()
                                / (SETTLE_MS as f32 / 1000.))
                                .min(1.);
                            if delta < 1. {
                                window.request_animation_frame();
                            }
                            px(top + (target_slot as f32 * ROW_PITCH - top) * ease_in_out(delta))
                        } else {
                            animated_y
                        };

                        let keys_for_drag = keys.clone();
                        let bounds_for_drag = self.bounds.clone();
                        let panel_for_drag = this_entity.clone();
                        let drag_key = key.clone();
                        let drag = drag_grip(&theme)
                            .id(SharedString::from(format!("type-filter-grip-{key}")))
                            .cursor(GRAB_CURSOR)
                            .hover(move |style| style.bg(btn_hover))
                            .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                                cx.stop_propagation();
                                panel_for_drag.update(cx, |panel, cx| {
                                    panel.settling = None;
                                    panel.drag = Some(PanelDrag {
                                        key: drag_key.clone(),
                                        keys: keys_for_drag.clone(),
                                        start: event.position,
                                        grab_y: f32::from(
                                            event.position.y - bounds_for_drag.get().top() - row_y,
                                        ),
                                        top: f32::from(row_y),
                                        target: original_slot,
                                        moved: false,
                                    });
                                    cx.notify();
                                });
                            })
                            // Keep the row's visibility toggle out of handle presses.
                            .on_click(|_, _, cx| cx.stop_propagation());

                        let this = this_entity.clone();
                        let click_key = key.clone();
                        filter_row(&key, visible, &theme)
                            .id(SharedString::from(format!("type-filter-{key}")))
                            .absolute()
                            .top(row_y)
                            .left_0()
                            .w_full()
                            .cursor(if is_dragging {
                                GRABBING_CURSOR
                            } else {
                                CursorStyle::PointingHand
                            })
                            .when(is_dragging, |row| row.shadow_md())
                            .hover(move |style| style.bg(btn_hover))
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                this.update(cx, |panel, cx| panel.toggle_visible(&click_key, cx));
                            })
                            .child(drag)
                    })),
            )
    }
}

/// Passive row visuals shared with the drag preview; interaction stays on the live row.
fn filter_row(key: &str, visible: bool, theme: &ClippiTheme) -> Div {
    let (icon, label) = filter_type_display(key).unwrap_or(("\u{e606}", key.to_string()));
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .h(px(ROW_HEIGHT))
        .px(px(4.))
        .rounded(px(4.))
        .child(
            div()
                .text_size(fs(12.))
                .font_family("iconfont")
                .text_color(if visible { theme.accent } else { theme.text_3 })
                .flex_shrink_0()
                .child(if visible { "\u{e61f}" } else { "\u{e831}" }),
        )
        .child(
            div()
                .ml(px(6.))
                .text_size(fs(12.))
                .font_family("iconfont")
                .text_color(if visible { theme.text_2 } else { theme.text_3 })
                .child(icon.to_string()),
        )
        .child(
            div()
                .ml(px(4.))
                .text_size(fs(11.))
                .flex_1()
                .text_color(if visible { theme.text_1 } else { theme.text_3 })
                .child(label),
        )
}
