//! Theme system for Clippi — color tokens matching the original Slint design.
//!
//! --- All colors are sourced from `ui/app.slint` and sub-components. ---
//! --- Tokens adapt between dark and light mode. ---

use gpui::{rgb, rgba, Rgba, WindowAppearance};

/// Semantic color palette for the Clippi UI.
/// Colors match the original Slint design pixel-for-pixel.
#[derive(Debug, Clone)]
pub struct ClippiTheme {
    // --- Window chrome ---
    pub bg: Rgba,
    pub surface: Rgba,
    pub surface_press: Rgba,
    pub titlebar_bg: Rgba,

    // --- Text ---
    pub text_1: Rgba,
    pub text_2: Rgba,
    pub text_3: Rgba,

    // --- Accent ---
    pub accent: Rgba,
    pub accent_soft: Rgba,

    // --- Semantic ---
    pub fav_color: Rgba,
    pub danger: Rgba,
    pub divider: Rgba,
    pub btn_hover: Rgba,

    // --- Tags ---
    pub tag_bg: Rgba,
    pub tag_text: Rgba,

    // --- Panels (floating) ---
    pub panel_surface: Rgba,
    pub panel_input_bg: Rgba,
    pub panel_sep_line: Rgba,

    // --- Toast ---
    pub toast_bg: Rgba,
}

impl ClippiTheme {
    pub fn dark() -> Self {
        Self {
            bg: rgb(0x191a1b),
            surface: rgb(0x232425),
            surface_press: rgb(0x3a3b3c),
            titlebar_bg: rgb(0x191a1b),

            text_1: rgb(0xeaebec),
            text_2: rgb(0x919496),
            text_3: rgb(0x5f6264),

            accent: rgb(0x7ecba3),
            accent_soft: rgb(0x1a2e24),

            fav_color: rgb(0xd8a155),
            danger: rgb(0xff5f57),
            divider: rgb(0x2b2c2d),
            btn_hover: rgb(0x2b2c2d),

            tag_bg: rgb(0x2c2e2f),
            tag_text: rgb(0xddf5e4),

            panel_surface: rgb(0x2c2d2e),
            panel_input_bg: rgb(0x1e1f20),
            panel_sep_line: rgba(0xffffff0d),

            toast_bg: rgba(0x3a3b3de8),
        }
    }

    pub fn light() -> Self {
        Self {
            bg: rgb(0xf2f3f8),
            surface: rgb(0xffffff),
            surface_press: rgb(0xebedf5),
            titlebar_bg: rgb(0xf2f3f8),

            text_1: rgb(0x1a1c2e),
            text_2: rgb(0x7c809a),
            text_3: rgb(0xb4b9ca),

            accent: rgb(0x6ab890),
            accent_soft: rgb(0xe8f5ee),

            fav_color: rgb(0xd8a155),
            danger: rgb(0xff5f57),
            divider: rgb(0xe6e8f0),
            btn_hover: rgb(0xf0f1f8),

            tag_bg: rgb(0xe6e8f0),
            tag_text: rgb(0x6c857c),

            panel_surface: rgb(0xffffff),
            panel_input_bg: rgb(0xf5f6fc),
            panel_sep_line: rgba(0x00000010),

            toast_bg: rgba(0xfffffff0),
        }
    }

    pub fn from_setting(theme: &str, appearance: Option<WindowAppearance>) -> Self {
        match theme {
            "light" => Self::light(),
            "dark" => Self::dark(),
            _ => match appearance {
                Some(WindowAppearance::Dark | WindowAppearance::VibrantDark) => Self::dark(),
                _ => Self::light(),
            },
        }
    }

    /// Accent color as a 22% opacity overlay (used for active filter button backgrounds).
    pub fn accent_overlay(&self) -> Rgba {
        if self.bg == rgb(0x191a1b) {
            rgba(0x7ecba322)
        } else {
            rgba(0x6ab89022)
        }
    }

    /// Accent color used behind search hits. Stronger than the generic
    /// overlay so short highlighted terms remain visible in dense previews.
    pub fn accent_highlight(&self) -> Rgba {
        if self.bg == rgb(0x191a1b) {
            rgba(0x7ecba366)
        } else {
            rgba(0x6ab8904d)
        }
    }

    pub fn accent_highlight_text(&self) -> Rgba {
        if self.bg == rgb(0x191a1b) {
            rgb(0xffffff)
        } else {
            rgb(0x000000)
        }
    }
}
