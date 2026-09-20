//! Stepped (detent-snapping) slider — a reusable settings control.
//!
//! Interaction model (the "drag feel"):
//! - pressing anywhere on the row grabs the knob and puts it exactly under the
//!   pointer;
//! - while the button is held the knob **follows the pointer continuously**
//!   (sub-detent motion), while the value snaps to the nearest detent — the
//!   host is told about the snapped detent via `on_change` and about the raw
//!   position via `on_preview` (visual feedback only);
//! - releasing snaps the knob onto the nearest stop with a short, monotonic
//!   settle that starts from exactly where the pointer left it. The
//!   interpolation is driven here rather than by an easing helper so the start
//!   point is always the rendered position — otherwise a quick click (release
//!   before the helper caught up) would first jump backwards, then forwards.
//!
//! Detents come in two flavours: *major* stops carry a label, *minor* stops sit
//! on the boundaries between them and are unlabelled, letting the slider be
//! nudged in smaller increments without cluttering the row. The labelled stops
//! define the geometry (first at the left end, last at the right end), so the
//! track spans the full width and every label sits on its own tick.
//!
//! Pointer-drag tracking mirrors the sidebar reorder pattern: a transparent
//! `canvas` captures the track bounds during layout and registers
//! capture-phase mouse-move / mouse-up listeners while the button is held.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::ui::font::fs;

pub type SliderChange = Rc<dyn Fn(usize, &mut Window, &mut App)>;
pub type SliderPreview = Rc<dyn Fn(&mut Window, &mut App)>;

/// Knob / track geometry (design px).
const TRACK_HEIGHT: f32 = 5.0;
/// Height of the track band (knob lives here).
const TRACK_BAND: f32 = 26.0;
/// Height of the label band below the track.
const LABEL_BAND: f32 = 24.0;
/// Width of a label cell: enough for the longest label, never wider than the
/// gap between neighbouring stops.
const LABEL_WIDTH: f32 = 56.0;
const KNOB_SIZE: f32 = 17.0;
const KNOB_SIZE_DRAGGING: f32 = 20.0;
/// Thickness of the knob's outer ring; the inner core fills the remainder.
const KNOB_RING: f32 = 2.5;
const MAJOR_TICK_W: f32 = 3.0;
const MAJOR_TICK_H: f32 = 8.0;
const MINOR_TICK_W: f32 = 2.0;
const MINOR_TICK_H: f32 = 6.0;
/// Length of the snap-settle after release. Short and monotonic: long enough
/// to read as "settling onto the stop", too short to look like a bounce.
const SETTLE_MS: u64 = 90;
/// How far the pointer must travel while held before the knob stops tracking
/// the snapped stop and starts following the pointer. Without it a press would
/// put the knob between two stops and only snap back on release.
const DRAG_THRESHOLD_PX: f32 = 4.0;

/// One stop on the track. `label == None` makes it a minor (unlabelled) stop.
#[derive(Clone)]
pub struct SliderDetent {
    pub label: Option<SharedString>,
    pub major: bool,
}

impl SliderDetent {
    pub fn major(label: impl Into<SharedString>) -> Self {
        Self {
            label: Some(label.into()),
            major: true,
        }
    }

    pub fn minor() -> Self {
        Self {
            label: None,
            major: false,
        }
    }
}

#[derive(Clone, Copy)]
pub struct SteppedSliderColors {
    /// Semantic accent (active label text, committed detent).
    pub accent: Rgba,
    /// Passed portion of the track — one step between track and accent.
    pub fill: Rgba,
    /// Untouched track.
    pub track_off: Rgba,
    /// Passed ticks.
    pub tick_passed: Rgba,
    /// Unpassed ticks — a touch lighter than the track so they read as stops.
    pub tick_off: Rgba,
    /// Knob outer ring — the brighter layer.
    pub knob_ring: Rgba,
    /// Knob inner core — the darker layer.
    pub knob_core: Rgba,
    /// Fill of an inactive number badge (see `badge_labels`).
    pub badge_bg: Rgba,
    pub text: Rgba,
}

/// Persistent interaction state for a [`SteppedSlider`].
///
/// The element re-renders (and drops its locals) on every pointer update, so
/// the "button held" flag, the live pointer position, the last committed detent
/// and the in-flight settle all live here — the same reason the settings panel
/// keeps a `HashMap` of toggle transition states.
#[derive(Clone)]
pub struct SliderDragState {
    dragging: Rc<Cell<bool>>,
    /// Raw pointer fraction (0..1) while dragging — the knob follows this.
    preview: Rc<Cell<f32>>,
    /// Last detent index reported through `on_change`.
    committed: Rc<Cell<usize>>,
    /// Knob fraction at the moment the button was released.
    released_from: Rc<Cell<f32>>,
    /// When the button was released, while the settle is in flight.
    released_at: Rc<Cell<Option<Instant>>>,
    /// Whether the pointer actually travelled during the current press. A plain
    /// click must land on the nearest stop at once; only a real drag gets the
    /// settle animation.
    moved: Rc<Cell<bool>>,
    /// Pointer fraction where the press started.
    press_origin: Rc<Cell<f32>>,
    /// Set once the press has travelled past [`DRAG_THRESHOLD_PX`]: from then
    /// on the knob follows the pointer instead of the snapped stop.
    following: Rc<Cell<bool>>,
}

impl SliderDragState {
    pub fn new(initial: usize) -> Self {
        Self {
            dragging: Rc::new(Cell::new(false)),
            preview: Rc::new(Cell::new(0.0)),
            committed: Rc::new(Cell::new(initial)),
            released_from: Rc::new(Cell::new(0.0)),
            released_at: Rc::new(Cell::new(None)),
            moved: Rc::new(Cell::new(false)),
            press_origin: Rc::new(Cell::new(0.0)),
            following: Rc::new(Cell::new(false)),
        }
    }

    /// Button pressed: grab the knob (no settle in flight). The knob stays on
    /// the snapped stop until the press turns into a real drag.
    fn begin(&self) {
        self.released_at.set(None);
        self.moved.set(false);
        self.following.set(false);
        self.dragging.set(true);
    }

    /// Record where the press started, so the drag threshold is measured from
    /// the point the user actually clicked.
    fn press_at(&self, fraction: f32) {
        self.preview.set(fraction);
        self.press_origin.set(fraction);
    }

    /// Whether the knob should follow the pointer for this position. Returns
    /// false (knob stays on the snapped stop) until the pointer has clearly
    /// moved away from the press point.
    fn should_follow(&self, fraction: f32, track_width: f32) -> bool {
        if !self.dragging.get() {
            return false;
        }
        if !self.following.get() {
            let threshold = DRAG_THRESHOLD_PX / track_width.max(1.0);
            if (fraction - self.press_origin.get()).abs() <= threshold {
                return false;
            }
            self.following.set(true);
            self.moved.set(true);
        }
        true
    }

    /// Button released. A drag settles from wherever the knob currently is; a
    /// plain click lands on the nearest stop immediately (no animation).
    fn release(&self) {
        self.dragging.set(false);
        if self.moved.get() {
            self.released_from.set(self.preview.get());
            self.released_at.set(Some(Instant::now()));
        } else {
            self.released_at.set(None);
        }
    }

    /// Knob fraction to draw for the given committed target.
    ///
    /// Returns the fraction plus whether another frame is still needed. The
    /// interpolation starts at `released_from` — the exact position that was on
    /// screen — so a quick click cannot snap backwards before settling.
    fn settle(&self, target: f32) -> (f32, bool) {
        if self.dragging.get() {
            return (
                if self.following.get() {
                    self.preview.get()
                } else {
                    target
                },
                false,
            );
        }
        let Some(started) = self.released_at.get() else {
            return (target, false);
        };
        let elapsed = started.elapsed().as_secs_f32();
        let duration = SETTLE_MS as f32 / 1000.0;
        // SETTLE_MS == 0 disables the settle entirely (instant positioning).
        let t = if duration <= 0.0 {
            1.0
        } else {
            (elapsed / duration).clamp(0.0, 1.0)
        };
        // Smoothstep: monotonic, so the knob only ever approaches the stop.
        let eased = t * t * (3.0 - 2.0 * t);
        let from = self.released_from.get();
        if t >= 1.0 {
            self.released_at.set(None);
            return (target, false);
        }
        (from + (target - from) * eased, true)
    }
}

#[derive(IntoElement)]
pub struct SteppedSlider {
    id: ElementId,
    detents: Vec<SliderDetent>,
    active: usize,
    colors: SteppedSliderColors,
    on_change: Option<SliderChange>,
    on_preview: Option<SliderPreview>,
    drag: Option<SliderDragState>,
    /// Draw the passed portion of the track. Off gives a bare scale — just
    /// ticks and the knob — for pickers where "how far along" is meaningless.
    show_fill: bool,
    /// Render labels as filled number badges (like the latest-hotkey slots)
    /// instead of plain text.
    badge_labels: bool,
    /// Padding reserved inside the row so the end labels — which are centred on
    /// the end stops — stay within the row instead of hanging outside it.
    label_inset: f32,
}

impl SteppedSlider {
    pub fn new(id: impl Into<ElementId>, detents: Vec<SliderDetent>) -> Self {
        Self {
            id: id.into(),
            detents,
            active: 0,
            colors: SteppedSliderColors {
                accent: rgb(0x4a9e6f),
                fill: rgb(0x4a9e6f),
                track_off: rgb(0x3a3b3c),
                tick_passed: rgb(0x4a9e6f),
                tick_off: rgb(0x55575a),
                knob_ring: rgb(0x8ed4a8),
                knob_core: rgb(0x35805a),
                badge_bg: rgb(0x2f3032),
                text: rgb(0xeaebec),
            },
            on_change: None,
            on_preview: None,
            drag: None,
            show_fill: true,
            badge_labels: false,
            label_inset: 0.0,
        }
    }

    pub fn value(mut self, active: usize) -> Self {
        self.active = active;
        self
    }

    pub fn colors(mut self, colors: SteppedSliderColors) -> Self {
        self.colors = colors;
        self
    }

    /// Called when the snapped detent changes (`index`, window, app).
    pub fn on_change(mut self, on_change: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(on_change));
        self
    }

    /// Called on every pointer update while dragging so the host can repaint;
    /// the knob position itself lives in [`SliderDragState`].
    pub fn on_preview(mut self, on_preview: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_preview = Some(Rc::new(on_preview));
        self
    }

    pub fn drag_state(mut self, drag: SliderDragState) -> Self {
        self.drag = Some(drag);
        self
    }

    /// Turn the passed-portion fill off for "bare scale" pickers.
    pub fn show_fill(mut self, show: bool) -> Self {
        self.show_fill = show;
        self
    }

    /// Draw each label as a filled number badge instead of plain text.
    pub fn badge_labels(mut self, badges: bool) -> Self {
        self.badge_labels = badges;
        self
    }

    /// Reserve room inside the row for the end labels (design px).
    pub fn label_inset(mut self, inset: f32) -> Self {
        self.label_inset = inset;
        self
    }
}

impl RenderOnce for SteppedSlider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            id,
            detents,
            active,
            colors,
            on_change,
            on_preview,
            drag,
            show_fill,
            badge_labels,
            label_inset,
        } = self;
        let count = detents.len().max(1);
        let active = active.min(count - 1);

        // Geometry: the labelled stops define the layout — they span the full
        // row (first at the left end, last at the right end) and the unlabelled
        // stops sit on the boundaries between them. Ticks, labels, the track and
        // the pointer math all read this one table, so they cannot disagree.
        let positions = detent_positions(&detents);
        let travel = travel_positions(&positions);
        let span_start = positions.first().copied().unwrap_or(0.0);
        let span_end = positions.last().copied().unwrap_or(1.0);
        // Travel fraction (0..1 between the end stops) → absolute position.
        let at = move |f: f32| span_start + (span_end - span_start) * f.clamp(0.0, 1.0);
        let active_travel = travel.get(active).copied().unwrap_or(0.0);

        let bounds: Rc<RefCell<Option<Bounds<Pixels>>>> = Rc::new(RefCell::new(None));
        // The caller owns the drag state so it survives re-renders; without one
        // the slider still snaps correctly, it just cannot animate.
        let state = drag.clone().unwrap_or_else(|| SliderDragState::new(active));
        // Only take the caller's value when nothing is in flight. While a press
        // or settle is running, `committed` is already ahead of the prop (the
        // host re-renders a frame later), and re-syncing would flip the settle
        // target back — which is what made a plain click wobble.
        if !state.dragging.get() && state.released_at.get().is_none() {
            state.committed.set(active);
        }
        let dragging = state.dragging.clone();
        let preview = state.preview.clone();
        let committed = state.committed.clone();

        // Both the settle and the bare-scale tick highlight follow the
        // *committed* stop, so neither can be dragged off by a lagging prop.
        let committed_index = committed.get().min(count - 1);
        let committed_travel = travel
            .get(committed_index)
            .copied()
            .unwrap_or(active_travel);

        // Knob fraction: straight from the pointer while the button is held (so
        // it is always under the cursor), then a short monotonic settle onto the
        // committed stop after release.
        let (knob_fraction, animating) = state.settle(committed_travel);

        let on_preview = Rc::new(on_preview);
        let interactive = {
            let on_change = Rc::new(on_change);
            let on_preview = on_preview.clone();
            let travel = travel.clone();
            Rc::new(move |fraction: f32, window: &mut Window, cx: &mut App| {
                let zone = nearest_detent(&travel, fraction);
                preview.set(fraction);
                if committed.get() != zone {
                    committed.set(zone);
                    if let Some(on_change) = on_change.as_ref().as_ref() {
                        on_change(zone, window, cx);
                    }
                }
                if let Some(on_preview) = on_preview.as_ref().as_ref() {
                    on_preview(window, cx);
                }
            })
        };

        // While the settle is in flight, ask the host for the next frame
        // (deferred, so this runs after the current render, not inside it).
        if animating {
            if let Some(on_preview) = on_preview.as_ref().as_ref() {
                let on_preview = on_preview.clone();
                window.defer(cx, move |window, cx| on_preview(window, cx));
            }
        }

        let knob_pos = at(knob_fraction);
        let knob_size = if dragging.get() {
            KNOB_SIZE_DRAGGING
        } else {
            KNOB_SIZE
        };

        // Outer wrapper carries the side inset; the inner row is the single,
        // padding-free coordinate frame for the pointer math, the track, the
        // ticks, the labels and the knob. Keeping the frame free of padding is
        // what guarantees a press lands exactly under the cursor: with padding
        // present, absolutely positioned children and the measuring canvas can
        // resolve against different boxes.
        div().w_full().px(px(label_inset)).child(
            div()
                .id(id)
                .relative()
                .w_full()
                .h(px(TRACK_BAND + LABEL_BAND))
                .cursor(CursorStyle::PointingHand)
                .on_mouse_down(MouseButton::Left, {
                    let bounds = bounds.clone();
                    let drag_state = state.clone();
                    let interactive = interactive.clone();
                    move |ev: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        drag_state.begin();
                        if let Some(fraction) =
                            fraction_at(&bounds, ev.position, span_start, span_end)
                        {
                            // Select the nearest stop right away; the knob stays on
                            // it unless the press turns into a drag.
                            drag_state.press_at(fraction);
                            interactive(fraction, window, cx);
                        }
                    }
                })
                // --- Passive track, inset to the end stops ---
                .child(
                    div()
                        .absolute()
                        .top(px((TRACK_BAND - TRACK_HEIGHT) / 2.0))
                        .left(relative(span_start))
                        .right(relative(1.0 - span_end))
                        .h(px(TRACK_HEIGHT))
                        .rounded(px(TRACK_HEIGHT / 2.0))
                        .bg(colors.track_off),
                )
                // --- Passed portion (omitted for bare-scale pickers) ---
                .when(show_fill, |row| {
                    row.child(
                        div()
                            .absolute()
                            .top(px((TRACK_BAND - TRACK_HEIGHT) / 2.0))
                            .left(relative(span_start))
                            .right(relative((1.0 - knob_pos).max(0.0)))
                            .h(px(TRACK_HEIGHT))
                            .rounded(px(TRACK_HEIGHT / 2.0))
                            .bg(colors.fill),
                    )
                })
                // --- Detent ticks: solid rounded bars straddling the track.
                // --- With a fill the passed stops light up as progress; on a bare
                // --- scale only the current stop is lit — the rest stay neutral.
                .children(detents.iter().enumerate().map(|(i, detent)| {
                    let offset = positions.get(i).copied().unwrap_or(0.0);
                    let lit = if show_fill {
                        offset <= knob_pos
                    } else {
                        i == committed_index
                    };
                    let (w, h) = if detent.major {
                        (MAJOR_TICK_W, MAJOR_TICK_H)
                    } else {
                        (MINOR_TICK_W, MINOR_TICK_H)
                    };
                    div()
                        .absolute()
                        .top(px((TRACK_BAND - h) / 2.0))
                        .left(relative(offset))
                        .ml(px(-w / 2.0))
                        .w(px(w))
                        .h(px(h))
                        .rounded(px(w / 2.0))
                        .bg(if lit {
                            colors.tick_passed
                        } else {
                            colors.tick_off
                        })
                }))
                // --- Labels: every label is centred on its own stop, read from the
                // --- same `positions` table as the ticks, so they cannot drift.
                // --- `label_inset` keeps the end labels inside the row. ---
                .children(detents.iter().enumerate().filter_map({
                    let interactive = interactive.clone();
                    let positions = positions.clone();
                    let travel = travel.clone();
                    move |(i, detent)| {
                        let label = detent.label.clone()?;
                        let is_active = i == committed_index;
                        let on_change = interactive.clone();
                        let travel_at = travel.get(i).copied().unwrap_or(0.0);
                        let position = positions.get(i).copied().unwrap_or(0.0);

                        let text = if badge_labels {
                            div()
                                .w(px(20.))
                                .h(px(20.))
                                .rounded(px(10.))
                                .bg(if is_active {
                                    colors.accent
                                } else {
                                    colors.badge_bg
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(fs(10.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(if is_active {
                                    rgb(0xffffff)
                                } else {
                                    colors.text
                                })
                                .child(label)
                        } else {
                            div()
                                .max_w_full()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_size(fs(11.))
                                .font_weight(if is_active {
                                    FontWeight::BOLD
                                } else {
                                    FontWeight::default()
                                })
                                .text_color(if is_active {
                                    colors.accent
                                } else {
                                    colors.text
                                })
                                .child(label)
                        };

                        Some(
                            div()
                                .absolute()
                                .top(px(TRACK_BAND + 2.0))
                                .left(relative(position))
                                .ml(px(-LABEL_WIDTH / 2.0))
                                .w(px(LABEL_WIDTH))
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(px(1.))
                                .cursor(CursorStyle::PointingHand)
                                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                    on_change(travel_at, window, cx);
                                })
                                .child(text),
                        )
                    }
                }))
                // --- Knob: bright outer ring around a darker core ---
                .child(
                    div()
                        .absolute()
                        .top(px((TRACK_BAND - knob_size) / 2.0))
                        .left(relative(knob_pos))
                        .ml(px(-knob_size / 2.0))
                        .w(px(knob_size))
                        .h(px(knob_size))
                        .rounded_full()
                        .bg(colors.knob_ring)
                        .shadow_md()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .w(px(knob_size - KNOB_RING * 2.0))
                                .h(px(knob_size - KNOB_RING * 2.0))
                                .rounded_full()
                                .bg(colors.knob_core),
                        ),
                )
                // --- Drag listeners: capture-move to follow, capture-up to settle ---
                .child(
                    canvas(
                        {
                            let bounds = bounds.clone();
                            move |bounds_box, _, _| *bounds.borrow_mut() = Some(bounds_box)
                        },
                        {
                            let bounds = bounds.clone();
                            let drag_state = state.clone();
                            let interactive = interactive.clone();
                            move |_bounds, _hitbox, window, _cx| {
                                window.on_mouse_event({
                                    let bounds = bounds.clone();
                                    let drag_state = drag_state.clone();
                                    let interactive = interactive.clone();
                                    move |ev: &MouseMoveEvent, phase, window, cx| {
                                        if phase != DispatchPhase::Capture
                                            || !drag_state.dragging.get()
                                        {
                                            return;
                                        }
                                        if ev.pressed_button != Some(MouseButton::Left) {
                                            // Button lost (released outside) — settle
                                            // from where the knob currently is.
                                            drag_state.release();
                                            return;
                                        }
                                        if let Some((fraction, width)) = fraction_and_width_at(
                                            &bounds,
                                            ev.position,
                                            span_start,
                                            span_end,
                                        ) {
                                            // Ignore tiny jitter: until the pointer
                                            // clearly departs the press point this
                                            // is still a click, so the knob stays
                                            // on the snapped stop.
                                            if drag_state.should_follow(fraction, width) {
                                                interactive(fraction, window, cx);
                                            }
                                        }
                                    }
                                });
                                window.on_mouse_event({
                                    let drag_state = drag_state.clone();
                                    let on_preview = on_preview.clone();
                                    move |ev: &MouseUpEvent, phase, window, cx| {
                                        if phase == DispatchPhase::Capture
                                            && ev.button == MouseButton::Left
                                            && drag_state.dragging.get()
                                        {
                                            // Settle starts from the exact position
                                            // the pointer left the knob at.
                                            drag_state.release();
                                            if let Some(on_preview) = on_preview.as_ref().as_ref() {
                                                on_preview(window, cx);
                                            }
                                        }
                                    }
                                });
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                ),
        )
    }
}

/// Absolute position (0..1 of the row width) of every detent.
///
/// The *labelled* stops define the geometry: they span the full row (first at
/// the left end, last at the right end) so the track uses the whole width, and
/// the unlabelled stops sit halfway between them. Ticks, labels, the track span
/// and the pointer math all read this one table.
fn detent_positions(detents: &[SliderDetent]) -> Vec<f32> {
    let majors: Vec<usize> = detents
        .iter()
        .enumerate()
        .filter(|(_, d)| d.major)
        .map(|(i, _)| i)
        .collect();
    if majors.is_empty() {
        let count = detents.len().max(1) as f32;
        return (0..detents.len())
            .map(|i| (i as f32 + 0.5) / count)
            .collect();
    }

    let gaps = majors.len().saturating_sub(1).max(1) as f32;
    let mut positions = vec![0.0_f32; detents.len()];
    for (k, &i) in majors.iter().enumerate() {
        positions[i] = k as f32 / gaps;
    }
    // Unlabelled stops are resolved after the labelled ones so they can
    // interpolate between their neighbours.
    for j in 0..detents.len() {
        if detents[j].major {
            continue;
        }
        let prev = majors.iter().rev().find(|&&m| m < j).copied();
        let next = majors.iter().find(|&&m| m > j).copied();
        positions[j] = match (prev, next) {
            (Some(p), Some(n)) => (positions[p] + positions[n]) / 2.0,
            (Some(p), None) => positions[p],
            (None, Some(n)) => positions[n],
            (None, None) => 0.5,
        };
    }
    positions
}

/// Travel fraction of each detent: 0 at the first stop, 1 at the last. Every
/// stored/compared fraction (pointer preview, transition goal, snapping) uses
/// this unit; only the render converts it back to an absolute position.
fn travel_positions(positions: &[f32]) -> Vec<f32> {
    let start = positions.first().copied().unwrap_or(0.0);
    let end = positions.last().copied().unwrap_or(1.0);
    let span = (end - start).max(f32::EPSILON);
    positions
        .iter()
        .map(|p| ((p - start) / span).clamp(0.0, 1.0))
        .collect()
}

/// Index of the detent closest to `fraction` (used for snapping).
fn nearest_detent(travel: &[f32], fraction: f32) -> usize {
    let mut best = 0usize;
    let mut best_err = f32::MAX;
    for (i, t) in travel.iter().enumerate() {
        let err = (t - fraction).abs();
        if err < best_err {
            best_err = err;
            best = i;
        }
    }
    best
}

/// Pointer position → travel fraction, clamped to the end stops so the knob can
/// never be dragged past them. `span_start`/`span_end` are the absolute
/// positions of the first/last stop. Returns `None` before the first layout
/// pass has captured the bounds.
fn fraction_at(
    bounds: &Rc<RefCell<Option<Bounds<Pixels>>>>,
    position: Point<Pixels>,
    span_start: f32,
    span_end: f32,
) -> Option<f32> {
    fraction_and_width_at(bounds, position, span_start, span_end).map(|(fraction, _)| fraction)
}

/// Same as [`fraction_at`], but also reports the track width so callers can
/// apply a pixel-based threshold (see `DRAG_THRESHOLD_PX`).
fn fraction_and_width_at(
    bounds: &Rc<RefCell<Option<Bounds<Pixels>>>>,
    position: Point<Pixels>,
    span_start: f32,
    span_end: f32,
) -> Option<(f32, f32)> {
    let held = bounds.borrow();
    let bounds = held.as_ref()?;
    let width = f32::from(bounds.size.width);
    if width <= 0.0 {
        return None;
    }
    let raw = (f32::from(position.x) - f32::from(bounds.origin.x)) / width;
    let span = (span_end - span_start).max(f32::EPSILON);
    Some((((raw - span_start) / span).clamp(0.0, 1.0), width))
}

#[cfg(test)]
mod tests {
    use super::{
        detent_positions, nearest_detent, travel_positions, SliderDetent, SliderDragState,
        SETTLE_MS,
    };

    /// Mirror what the element's pointer handler does: once the press has
    /// travelled past the threshold the raw position is recorded, otherwise the
    /// knob stays on the snapped stop.
    fn pointer(state: &SliderDragState, fraction: f32, width: f32) {
        if state.should_follow(fraction, width) {
            state.preview.set(fraction);
        }
    }

    /// The font-size layout: 4 labelled stops with an unlabelled stop between
    /// each pair.
    fn font_detents() -> Vec<SliderDetent> {
        vec![
            SliderDetent::major("紧凑"),
            SliderDetent::minor(),
            SliderDetent::major("标准"),
            SliderDetent::minor(),
            SliderDetent::major("宽松"),
            SliderDetent::minor(),
            SliderDetent::major("特大"),
        ]
    }

    /// Labelled stops define the geometry: first at the left end, last at the
    /// right end, the rest evenly between — so the track spans the full row.
    #[test]
    fn labelled_stops_span_the_full_width() {
        let detents = font_detents();
        let positions = detent_positions(&detents);
        let majors: Vec<usize> = detents
            .iter()
            .enumerate()
            .filter(|(_, d)| d.major)
            .map(|(i, _)| i)
            .collect();

        for (k, &i) in majors.iter().enumerate() {
            let expected = k as f32 / (majors.len() - 1) as f32;
            assert!(
                (positions[i] - expected).abs() < 1e-5,
                "major {k}: position={} expected={expected}",
                positions[i]
            );
        }
        // Unlabelled stops sit exactly between their neighbours.
        assert!((positions[1] - 1.0 / 6.0).abs() < 1e-5);
        assert!((positions[3] - 0.5).abs() < 1e-5);
        assert!((positions[5] - 5.0 / 6.0).abs() < 1e-5);
    }

    /// Stops are evenly spaced and travel fractions round-trip through the
    /// absolute positions: `to_absolute(travel[i]) == positions[i]`. A
    /// mismatch here is what put the knob half a step away from its tick.
    #[test]
    fn travel_fractions_round_trip_to_stop_positions() {
        for detents in [
            font_detents(),
            vec![SliderDetent::major("a"), SliderDetent::major("b")],
            vec![
                SliderDetent::major("a"),
                SliderDetent::minor(),
                SliderDetent::major("b"),
                SliderDetent::minor(),
                SliderDetent::major("c"),
            ],
        ] {
            let positions = detent_positions(&detents);
            let travel = travel_positions(&positions);
            let start = positions[0];
            let end = positions[positions.len() - 1];

            assert!((travel[0] - 0.0).abs() < 1e-5);
            assert!((travel[travel.len() - 1] - 1.0).abs() < 1e-5);

            for (i, &t) in travel.iter().enumerate() {
                let absolute = start + (end - start) * t;
                assert!(
                    (absolute - positions[i]).abs() < 1e-5,
                    "i={i}: absolute={absolute} position={}",
                    positions[i]
                );
            }
        }
    }

    /// Snapping round-trips, and free positions land on the nearest stop.
    #[test]
    fn snapping_round_trips_and_picks_the_nearest_stop() {
        let positions = detent_positions(&font_detents());
        let travel = travel_positions(&positions);

        for (i, &t) in travel.iter().enumerate() {
            assert_eq!(nearest_detent(&travel, t), i, "round trip i={i}");
        }

        // 0 = first stop, 1 = last stop.
        assert_eq!(nearest_detent(&travel, -0.4), 0);
        assert_eq!(nearest_detent(&travel, 0.02), 0);
        assert_eq!(nearest_detent(&travel, 0.18), 1);
        assert_eq!(nearest_detent(&travel, 0.99), travel.len() - 1);
        assert_eq!(nearest_detent(&travel, 5.0), travel.len() - 1);
    }

    /// While dragging the knob must be exactly where the pointer is — no easing,
    /// so the cursor and the knob cannot drift apart.
    #[test]
    fn dragging_uses_the_pointer_position_verbatim() {
        let state = SliderDragState::new(0);
        state.begin();
        state.press_at(0.40);

        // Inside the threshold this is still a click: the knob stays snapped.
        pointer(&state, 0.41, 300.0);
        assert_eq!(state.settle(0.9).0, 0.9);

        // Past it the knob tracks the pointer verbatim.
        pointer(&state, 0.50, 300.0);
        assert_eq!(state.settle(0.9).0, 0.50);
    }

    /// A quick click must settle from the position that was on screen. If the
    /// settle instead restarted from some stale value the knob would visibly
    /// swing back before reaching the stop (the reported "左右晃动").
    #[test]
    fn settle_starts_at_the_released_position_and_never_overshoots() {
        let state = SliderDragState::new(0);
        state.begin();
        state.press_at(0.7);
        // Moving well past the threshold is what turns a press into a drag.
        assert!(state.should_follow(0.8, 300.0));
        state.release(); // released_from = 0.7

        let target = 0.2;
        let (first, animating) = state.settle(target);
        assert!(
            (first - 0.7).abs() < 0.2,
            "settle must start next to the release point, got {first}"
        );
        assert!(animating, "the settle should still be in flight");

        // Sampling later must land exactly on the stop, never past it.
        std::thread::sleep(std::time::Duration::from_millis(SETTLE_MS + 25));
        let (done, animating) = state.settle(target);
        assert!(!animating, "settle should have finished");
        assert!((done - target).abs() < 1e-4, "settled at {done}");
    }

    /// A plain click has no travel to animate: it must land on the nearest stop
    /// immediately, with no intermediate frames.
    #[test]
    fn a_plain_click_lands_on_the_stop_without_animating() {
        let state = SliderDragState::new(0);
        state.begin();
        state.preview.set(0.7);
        state.release(); // never moved

        let (value, animating) = state.settle(0.2);
        assert!(!animating, "a click must not animate");
        assert!((value - 0.2).abs() < 1e-6, "landed at {value}");
    }

    /// A drag keeps the settle: the knob eases from the release point.
    #[test]
    fn a_drag_settles_from_the_release_point() {
        let state = SliderDragState::new(0);
        state.begin();
        state.press_at(0.7);
        assert!(state.should_follow(0.8, 300.0));
        state.release();

        let (value, animating) = state.settle(0.2);
        assert!(animating, "a drag should animate");
        assert!(
            (value - 0.7).abs() < 0.2,
            "settle must start at the release point, got {value}"
        );
    }

    /// The movement flag is per press: a click after a drag is a click again.
    #[test]
    fn press_state_resets_between_gestures() {
        let state = SliderDragState::new(0);
        state.begin();
        state.press_at(0.2);
        assert!(state.should_follow(0.6, 300.0));
        state.release();

        state.begin();
        state.press_at(0.4);
        state.release();
        let (_, animating) = state.settle(0.4);
        assert!(
            !animating,
            "the second press is a click and must not animate"
        );
    }

    /// A press only starts following the pointer once it has travelled past the
    /// threshold — before that the knob stays on the snapped stop, which is what
    /// keeps a click from landing between two stops.
    #[test]
    fn a_press_must_pass_the_threshold_before_it_follows() {
        let state = SliderDragState::new(0);
        state.begin();
        state.press_at(0.50);

        // 1px on a 300px track is inside the threshold: still a click.
        assert!(!state.should_follow(0.5033, 300.0));
        let (value, animating) = state.settle(0.25);
        assert!(!animating);
        assert!((value - 0.25).abs() < 1e-6, "knob must stay on the stop");

        // Past the threshold the knob starts tracking the pointer.
        pointer(&state, 0.56, 300.0);
        let (value, animating) = state.settle(0.25);
        assert!(!animating, "no settle while dragging");
        assert!(
            (value - 0.56).abs() < 1e-6,
            "knob should follow, got {value}"
        );
    }
}
