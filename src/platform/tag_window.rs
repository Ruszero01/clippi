//! Native attachment for the tag sidebar. Movement never uses the UI poll loop.

#[cfg(target_os = "macos")]
#[path = "tag_window_macos.rs"]
mod implementation;
#[cfg(target_os = "windows")]
#[path = "tag_window_windows.rs"]
mod implementation;
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use implementation::Attachment;

pub const WIDTH: f32 = 36.;
pub const TOP: f32 = 65.;
pub const BOTTOM: f32 = 8.;
pub const ROW_HEIGHT: f32 = 24.;
/// Transparent margin around the tag strip. Each tag paints its own stroke and a
/// soft drop shadow, which needs room to the left, above and below the strip; the
/// strip's right edge still meets the main panel's stroke exactly.
pub const SHADOW: f32 = 6.;

/// Height of the visible tag strip that fits between `TOP` and `BOTTOM` inside a
/// panel of `panel_height` logical pixels.
pub fn sidebar_height(panel_height: f32, rows: usize) -> f32 {
    let available = (panel_height - TOP - BOTTOM).max(0.);
    (available / ROW_HEIGHT).floor().min(rows as f32) * ROW_HEIGHT
}

/// Native surface size for a strip of `strip_height` logical pixels.
pub fn window_size(strip_height: f32) -> (f32, f32) {
    (WIDTH + SHADOW, strip_height + SHADOW * 2.)
}

/// Client geometry of the owner for an in-flight native move or resize.
///
/// `proposed` is the owner's proposed window rectangle, `origin_offset` the
/// distance from that rectangle's origin to the client origin, and
/// `vertical_inset` the window height minus the client height. The inset stays
/// constant while the user drags or resizes, so the tag surface can be placed
/// before the owner commits the rectangle.
#[cfg(any(target_os = "windows", test))]
pub fn client_geometry(
    proposed: (i32, i32, i32, i32),
    origin_offset: (i32, i32),
    vertical_inset: i32,
) -> ((i32, i32), i32) {
    (
        (proposed.0 + origin_offset.0, proposed.1 + origin_offset.1),
        ((proposed.3 - proposed.1) - vertical_inset).max(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_geometry_handles_empty_small_and_overflowing_windows() {
        assert_eq!(sidebar_height(480., 0), 0.);
        assert_eq!(sidebar_height(480., 3), 72.);
        assert_eq!(sidebar_height(480., 100), 384.);
        assert_eq!(sidebar_height(60., 3), 0.);
    }

    #[test]
    fn tag_surface_keeps_a_margin_for_the_tag_shadows() {
        let (width, height) = window_size(96.);
        assert_eq!(width, WIDTH + SHADOW);
        assert_eq!(height, 96. + SHADOW * 2.);
    }

    #[test]
    fn proposed_window_rect_maps_to_the_owner_client_origin() {
        // A frame inset of (2, 2) and a 4px vertical inset, as on a DPI-scaled
        // Windows window: the tag surface must follow the client origin, not the
        // window origin, so it keeps meeting the main panel's left stroke.
        assert_eq!(
            client_geometry((100, 50, 700, 650), (2, 2), 4),
            ((102, 52), 596)
        );
        // Negative multi-monitor coordinates and a shrinking top edge resize.
        assert_eq!(
            client_geometry((-1000, 300, -400, 780), (2, 2), 4),
            ((-998, 302), 476)
        );
        // A window shorter than its frame insets must clamp instead of underflow.
        assert_eq!(client_geometry((0, 0, 100, 2), (1, 2), 8), ((1, 2), 0));
    }
}
