//! Animated toggle switch — reusable component for settings UI.
//!
//! Uses `gpui_transitions::use_keyed_transition` for a smooth 200ms
//! knob slide between OFF (left, 2px) and ON (right, 20px) positions.
//!
//! The `key` distinguishes toggles so transitions are independent;
//! pass a unique label for each instance.

use std::collections::HashMap;
use std::time::Duration;

use gpui::*;
use gpui_transitions::WindowUseTransition;

pub fn render_toggle(
    value: bool,
    key: &str,
    accent: Rgba,
    track_off: Rgba,
    states: &mut HashMap<String, (bool, u64)>,
    window: &mut Window,
    cx: &mut App,
    on_toggle: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let entry = states.entry(key.to_string()).or_insert((value, 0));
    if entry.0 != value {
        entry.0 = value;
        entry.1 += 1;
    }

    // Start from opposite side, then animate to target.
    // This mirrors the sidebar tag animation pattern.
    let (initial_x, target_x) = if value {
        (2.0, 20.0)
    } else {
        (20.0, 2.0)
    };
    let hash_key = key
        .bytes()
        .fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64));
    let transition_key = hash_key.wrapping_add(entry.1 << 32);

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

    div()
        .w(px(40.))
        .h(px(22.))
        .rounded(px(11.))
        .bg(if value { accent } else { track_off })
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

fn ease_in_out(delta: f32) -> f32 {
    if delta < 0.5 {
        2.0 * delta * delta
    } else {
        1.0 - (-2.0 * delta + 2.0).powi(2) / 2.0
    }
}
