//! Sidebar - tag navigation sidebar matching Slint `SideTagBar.slint`.
//!
//! --- Slint behavior: ---
//! --- - width 56px, rows placed every 24px; ---
//! --- - checked tags slide to x=0 and are fully opaque; ---
//! --- - unchecked pinned tags remain visible with dimmed text; ---
//! --- - unchecked, unpinned tags slide slightly right and fade out; ---
//! --- - left click toggles a visible tag filter, right click toggles pin. ---

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_transitions::WindowUseTransition;

use crate::core::types::TagInfo;
use crate::state::app::AppState;

use super::clipboard_list::ClipboardListView;
use super::theme::ClippiTheme;

/// Sidebar entity for displaying and managing tags.
pub struct Sidebar {
    state: Entity<AppState>,
    list_view: Entity<ClipboardListView>,
    rendered_tag_ids: Vec<i64>,
    unchecked_unpinned_since: HashMap<i64, Instant>,
    transition_generations: HashMap<i64, u64>,
    dark_mode: bool,
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
        }
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

        let display_tags = ordered_sidebar_tags(
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

        let dark = self.dark_mode;
        let text_1 = if dark { rgb(0xeaebec) } else { rgb(0x1a1c2e) };
        let row_bg_default = if dark { rgb(0x2a2b2e) } else { rgb(0xf5f6fa) };
        let row_bg_hover = if dark { rgb(0x353638) } else { rgb(0xeceef4) };
        let state_for_click = self.state.clone();
        let list_for_click = self.list_view.clone();
        let sidebar_for_notify = cx.entity().clone();
        let duration = Duration::from_millis(250);

        div()
            .w(px(56.))
            .bg(rgba(0x00000000))
            .flex()
            .flex_col()
            .gap(px(2.))
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
                let state_for_left = state_for_click.clone();
                let list_for_left = list_for_click.clone();
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

                let row = div()
                    .ml(row_x)
                    .w(px(56.))
                    .h(px(22.))
                    .opacity(opacity)
                    .rounded(px(4.))
                    .bg(row_bg_default)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.))
                    .cursor(if interactable {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::Arrow
                    })
                    .when(interactable, move |row| {
                        row.hover(move |style| style.bg(row_bg_hover))
                    })
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
                            .child(label),
                    );

                if interactable {
                    row.on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                        let items = state_for_left.update(cx, |state, _cx| {
                            state.toggle_tag_filter(tag_id);
                            state.items.clone()
                        });
                        list_for_left.update(cx, |list, cx| list.set_items(items, cx));
                    })
                    .on_mouse_down(
                        MouseButton::Right,
                        move |_ev, _window, cx| {
                            state_for_right.update(cx, |state, _cx| {
                                state.toggle_pinned_tag(tag_id);
                            });
                            sidebar_for_right.update(cx, |_sidebar, cx| cx.notify());
                        },
                    )
                } else {
                    row
                }
            }))
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
    let mut ordered = Vec::new();

    for pinned_id in pinned_tag_ids {
        if let Some(tag) = tags.iter().find(|tag| tag.id == *pinned_id) {
            ordered.push(tag.clone());
        }
    }

    for tag in tags {
        if active_tag_ids.contains(&tag.id) && !pinned_tag_ids.contains(&tag.id) {
            ordered.push(tag.clone());
        }
    }

    for tag in tags {
        if unchecked_unpinned_since.contains_key(&tag.id)
            && !active_tag_ids.contains(&tag.id)
            && !pinned_tag_ids.contains(&tag.id)
        {
            ordered.push(tag.clone());
        }
    }

    ordered
}

fn ease_in_out(delta: f32) -> f32 {
    if delta < 0.5 {
        2.0 * delta * delta
    } else {
        1.0 - (-2.0 * delta + 2.0).powi(2) / 2.0
    }
}
