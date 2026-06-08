//! Animated toggle switch — reusable component for settings UI.
//!
//! Uses `gpui_transitions::use_keyed_transition` for a smooth 200ms
//! --- knob slide between OFF (left, 2px) and ON (right, 20px) positions. ---
//!
//! --- The `key` distinguishes toggles so transitions are independent; ---
//! pass a unique label for each instance.

use std::collections::HashMap;
use std::time::Duration;

use gpui::*;
use gpui_transitions::WindowUseTransition;

#[derive(Clone, Copy)]
pub(crate) struct ToggleTransitionState {
    value: bool,
    generation: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct ToggleColors {
    pub accent: Rgba,
    pub track_off: Rgba,
}

pub fn render_toggle(
    value: bool,
    key: &str,
    colors: ToggleColors,
    states: &mut HashMap<String, ToggleTransitionState>,
    window: &mut Window,
    cx: &mut App,
    on_toggle: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (previous_value, generation, changed) = match states.get_mut(key) {
        Some(state) => {
            let previous_value = state.value;
            let changed = previous_value != value;
            if changed {
                state.value = value;
                state.generation += 1;
            }
            (previous_value, state.generation, changed)
        }
        None => {
            states.insert(
                key.to_string(),
                ToggleTransitionState {
                    value,
                    generation: 0,
                },
            );
            (value, 0, false)
        }
    };

    let target_x = toggle_x(value);
    let initial_x = if changed {
        toggle_x(previous_value)
    } else {
        target_x
    };
    let target_bg = toggle_bg(value, colors.accent, colors.track_off);
    let initial_bg = if changed {
        toggle_bg(previous_value, colors.accent, colors.track_off)
    } else {
        target_bg
    };
    let hash_key = key
        .bytes()
        .fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64));
    let transition_key = hash_key.wrapping_add(generation << 32);

    let knob_transition = window
        .use_keyed_transition(
            ("settings-toggle-knob", transition_key),
            cx,
            Duration::from_millis(200),
            move |_, _| initial_x,
        )
        .with_easing(ease_in_out);
    knob_transition.update(cx, |value, cx| {
        *value = target_x;
        cx.notify();
    });
    let knob_x = *knob_transition.evaluate(window, cx);

    let bg_transition = window
        .use_keyed_transition(
            ("settings-toggle-bg", transition_key),
            cx,
            Duration::from_millis(200),
            move |_, _| initial_bg,
        )
        .with_easing(ease_in_out);
    bg_transition.update(cx, |value, cx| {
        *value = target_bg;
        cx.notify();
    });
    let track_bg = *bg_transition.evaluate(window, cx);

    div()
        .w(px(40.))
        .h(px(22.))
        .rounded(px(11.))
        .bg(track_bg)
        .flex()
        .items_center()
        .cursor(CursorStyle::PointingHand)
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            on_toggle(_window, cx);
        })
        .child(
            div()
                .w(px(18.))
                .h(px(18.))
                .rounded(px(9.))
                .bg(rgb(0xffffff))
                .ml(px(knob_x)),
        )
}

fn toggle_x(value: bool) -> f32 {
    if value {
        20.0
    } else {
        2.0
    }
}

fn toggle_bg(value: bool, accent: Rgba, track_off: Rgba) -> Rgba {
    if value {
        accent
    } else {
        track_off
    }
}

fn ease_in_out(delta: f32) -> f32 {
    if delta < 0.5 {
        2.0 * delta * delta
    } else {
        1.0 - (-2.0 * delta + 2.0).powi(2) / 2.0
    }
}
