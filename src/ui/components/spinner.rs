//! Small reusable activity spinner used by transfer progress indicators.

use std::time::Duration;

use gpui::*;

pub fn activity_spinner(
    animation_id: impl Into<SharedString>,
    color: Rgba,
    size: f32,
) -> AnyElement {
    div()
        .relative()
        .size(px(size))
        .with_animation(
            animation_id.into(),
            Animation::new(Duration::from_millis(800)).repeat(),
            move |spinner, delta| {
                let dot_size = (size * 0.22).clamp(2.5, 3.5);
                let travel = size - dot_size;
                let mid = travel / 2.;
                let near = travel * 0.15;
                let far = travel * 0.85;
                let positions = [
                    (mid, 0.),
                    (far, near),
                    (travel, mid),
                    (far, far),
                    (mid, travel),
                    (near, far),
                    (0., mid),
                    (near, near),
                ];
                let index = ((delta * positions.len() as f32) as usize).min(positions.len() - 1);
                let (x, y) = positions[index];
                spinner
                    .child(
                        div()
                            .absolute()
                            .inset(px(0.))
                            .rounded(px(size / 2.))
                            .border(px(1.))
                            .border_color(color)
                            .opacity(0.42),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(x))
                            .top(px(y))
                            .size(px(dot_size))
                            .rounded(px(dot_size / 2.))
                            .bg(color),
                    )
            },
        )
        .into_any_element()
}
