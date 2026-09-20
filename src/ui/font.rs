//! Central font system — one scale multiplier + one custom family for the whole UI.
//!
//! The app's text sizes are authored as fixed design values (e.g. 13 px body,
//! 11 px meta). This module exposes those design values multiplied by a single
//! live scale, so every role (title / body / meta / tiny) grows and shrinks
//! *together* — the relative type hierarchy is preserved instead of flattening
//! everything to one size.
//!
//! Two scaling channels, both driven by the same multiplier so they stay in
//! lockstep:
//! - Clippi's own GPUI elements call [`fs`]/[`lh`]/[`sc`] at render time.
//! - gpui-component widgets (inputs, buttons, …) scale through the theme rem
//!   base set in [`apply_to_global_theme`] / [`sync_window_rem_size`].
//!
//! The font family is applied through `Theme::font_family`, which the
//! gpui-component `Root` view already cascades to every child. Because the
//! family is inherited (iconfont glyphs set theirs locally and are unaffected),
//! a single family choice re-skins the whole UI, and Chinese/English both
//! render: DirectWrite/CoreText resolve any missing glyphs through the system
//! fallback chain, so a Latin-only font still shows CJK and vice versa.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::RwLock;

use gpui::{px, App, Pixels, SharedString, Window};

/// Live scale multiplier, stored as f32 bits. Read/written only on the GPUI
/// main thread (render + event handlers), so a relaxed atomic is sufficient.
static SCALE: AtomicU32 = AtomicU32::new(1.0_f32.to_bits());

/// Live font family ("" == system default). Guarded by an RwLock because it is
/// read during render and written from the settings callback.
static FAMILY: RwLock<String> = RwLock::new(String::new());

/// Slider detents: 4 labelled "major" stops (compact / standard / large /
/// xlarge) with an unlabelled minor stop between each pair, so the slider can
/// be nudged in smaller increments without cluttering the labels.
pub const FONT_SIZE_DETENTS: &[f32] = &[0.92, 0.96, 1.00, 1.075, 1.15, 1.225, 1.30];

/// Indices into [`FONT_SIZE_DETENTS`] that carry a text label.
pub const FONT_SIZE_MAJOR_DETENTS: &[usize] = &[0, 2, 4, 6];

/// Detent index whose scale is the default (1.0 = standard).
pub const FONT_SIZE_DEFAULT_DETENT: usize = 2;

/// Legacy level names → scale, for configs written before the numeric scale.
pub fn level_scale(level: &str) -> f32 {
    match level {
        "compact" => 0.92,
        "large" => 1.15,
        "xlarge" => 1.30,
        _ => 1.0,
    }
}

/// Nearest detent index for an arbitrary scale.
pub fn detent_for_scale(scale: f32) -> usize {
    FONT_SIZE_DETENTS
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            (*a - scale)
                .abs()
                .partial_cmp(&(*b - scale).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(FONT_SIZE_DEFAULT_DETENT)
}

/// gpui-component's default rem base. Kept in sync with the `Theme::font_size`
/// it resets to, so scaling is purely the multiplier.
const REM_BASE: f32 = 16.0;

/// gpui's special family name for the platform UI font.
pub const SYSTEM_UI_FONT: &str = ".SystemUIFont";

/// Monospace family for the styled rich-text preview.
///
/// The preview deliberately opts out of the UI family so extracted document
/// spans keep one fixed, column-aligned face, which is why it names a family
/// here instead of reading the theme. `Consolas` ships only with Windows and
/// Office: CoreText does not know it, so on macOS the preview would fall
/// through to a proportional fallback and misalign tab-separated cells.
#[cfg(target_os = "windows")]
pub const PREVIEW_FONT_FAMILY: &str = "Consolas";
#[cfg(target_os = "macos")]
pub const PREVIEW_FONT_FAMILY: &str = "Menlo";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const PREVIEW_FONT_FAMILY: &str = "monospace";

/// Current global UI scale multiplier.
#[inline]
pub fn scale() -> f32 {
    f32::from_bits(SCALE.load(Ordering::Relaxed))
}

/// Apply a raw scale multiplier (clamped to a sane range so a corrupted config
/// can never blow up layout).
pub fn apply_scale(scale: f32) {
    // `f32::clamp` returns NaN unchanged, and a NaN multiplier turns every
    // scaled size into NaN, so non-finite values fall back to the default.
    let scale = if scale.is_finite() {
        scale.clamp(0.8, 1.6)
    } else {
        1.0
    };
    SCALE.store(scale.to_bits(), Ordering::Relaxed);
}

/// Current stored family ("" == system default).
pub fn family() -> String {
    FAMILY.read().expect("font family lock").clone()
}

/// Update the live family ("" resets to the system UI font).
pub fn set_family(family: &str) {
    *FAMILY.write().expect("font family lock") = family.to_string();
}

/// Scale a design-pixel font size through the live multiplier. Use for every
/// `text_size`; the roles keep their authored ratios because one multiplier is
/// applied to all of them.
#[inline]
pub fn fs(base: f32) -> Pixels {
    px(base * scale())
}

/// Same as [`fs`] but reads clearly at explicit `line_height` call sites.
#[inline]
pub fn lh(base: f32) -> Pixels {
    px(base * scale())
}

/// Scale a design-pixel *layout* length (padding, gap, box size, corner radius)
/// so it zooms in lockstep with the text it frames. Used to uniformly zoom
/// subtrees (the clipboard card) where a partial scale would clip content.
#[inline]
pub fn sc(base: f32) -> Pixels {
    px(base * scale())
}

/// Scale a plain f32 used in layout arithmetic (measured widths, height buckets)
/// without materialising a [`Pixels`].
#[inline]
pub fn sx(base: f32) -> f32 {
    base * scale()
}

/// rem base for gpui-component widgets, keeping them in lockstep with [`fs`].
#[inline]
pub fn rem_base() -> Pixels {
    px(REM_BASE * scale())
}

/// Push the resolved size (rem base) and family into the gpui-component global
/// theme. Call at startup, on any setting change, and *after* every full
/// `Theme::change` rebuild (a rebuild resets these fields to defaults).
pub fn apply_to_global_theme(cx: &mut App) {
    let current = family();
    let resolved: SharedString = if current.trim().is_empty() {
        SYSTEM_UI_FONT.into()
    } else {
        current.trim().to_string().into()
    };
    let theme = gpui_component::Theme::global_mut(cx);
    theme.font_size = rem_base();
    theme.font_family = resolved;
}

/// Keep the window rem size current. gpui-component's `Root` also does this on
/// its own renders, but Clippi's views re-render independently of `Root`, so
/// every root render refreshes it for the whole window.
pub fn sync_window_rem_size(window: &mut Window) {
    window.set_rem_size(rem_base());
}

/// The system's installed font family names, de-duplicated and sorted, for the
/// font picker. Always includes the system-default sentinel at the front.
pub fn available_families(cx: &App) -> Vec<String> {
    let mut names = cx.text_system().all_font_names();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}
