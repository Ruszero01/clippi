//! Unified sensitive-content preview rendering.
//!
//! Renders a `Vec<SensitivePreviewPart>`: plain fragments in the normal text
//! colour, masked fragments in a dimmed colour.  Search highlights are only
//! applied to visible prefix/suffix — the mask and hidden content are never
//! passed to the highlighter.

use gpui::{
    div, prelude::FluentBuilder, px, rgb, rgba, FontWeight, IntoElement, ParentElement, RenderOnce,
    Styled, Window,
};

use crate::core::secret::{MaskedValue, SensitivePreviewPart};

/// A lightweight `RenderOnce` component that renders structured
/// `SensitivePreviewPart` segments with uniform styling.
#[derive(IntoElement)]
pub struct SensitiveText {
    parts: Vec<SensitivePreviewPart>,
    search_terms: Vec<String>,
    text_color: gpui::Rgba,
    mask_color: gpui::Rgba,
    highlight_bg: gpui::Rgba,
    highlight_text: gpui::Rgba,
    font_size: f32,
    font_weight: Option<FontWeight>,
}

impl SensitiveText {
    pub fn new(parts: Vec<SensitivePreviewPart>) -> Self {
        Self {
            parts,
            search_terms: Vec::new(),
            text_color: rgb(0x000000),
            mask_color: rgb(0x000000),
            highlight_bg: rgba(0x00000000),
            highlight_text: rgb(0x000000),
            font_size: 13.0,
            font_weight: Some(FontWeight::BOLD),
        }
    }

    pub fn search_terms(mut self, terms: Vec<String>) -> Self {
        self.search_terms = terms;
        self
    }

    pub fn text_color(mut self, color: gpui::Rgba) -> Self {
        self.text_color = color;
        self
    }

    pub fn mask_color(mut self, color: gpui::Rgba) -> Self {
        self.mask_color = color;
        self
    }

    pub fn highlight_bg(mut self, color: gpui::Rgba) -> Self {
        self.highlight_bg = color;
        self
    }

    pub fn highlight_text(mut self, color: gpui::Rgba) -> Self {
        self.highlight_text = color;
        self
    }

    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }

    pub fn font_weight(mut self, weight: FontWeight) -> Self {
        self.font_weight = Some(weight);
        self
    }
}

impl RenderOnce for SensitiveText {
    fn render(self, _window: &mut Window, _cx: &mut gpui::App) -> impl IntoElement {
        let text_color = self.text_color;
        let mask_color = self.mask_color;
        let highlight_bg = self.highlight_bg;
        let highlight_text = self.highlight_text;
        let font_size = self.font_size;
        let font_weight = self.font_weight;
        let search_terms = self.search_terms;

        div()
            .flex()
            .flex_row()
            .items_center()
            .overflow_hidden()
            .children(self.parts.into_iter().map(move |part| {
                let terms = search_terms.clone();
                match part {
                    SensitivePreviewPart::Plain(text) => div()
                        .text_size(px(font_size))
                        .when_some(font_weight, |this, w| this.font_weight(w))
                        .text_color(text_color)
                        .child(crate::ui::search_highlight::render_highlighted_inline(
                            text,
                            &terms,
                            text_color,
                            highlight_bg,
                            highlight_text,
                            font_size,
                            font_weight,
                        ))
                        .into_any_element(),
                    SensitivePreviewPart::Masked(MaskedValue {
                        prefix,
                        mask,
                        suffix,
                    }) => div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .text_size(px(font_size))
                        .when_some(font_weight, |this, w| this.font_weight(w))
                        .child(crate::ui::search_highlight::render_highlighted_inline(
                            prefix,
                            &terms,
                            text_color,
                            highlight_bg,
                            highlight_text,
                            font_size,
                            font_weight,
                        ))
                        .child(
                            div()
                                .text_size(px(font_size))
                                .when_some(font_weight, |this, w| this.font_weight(w))
                                .text_color(mask_color)
                                .child(mask.to_string()),
                        )
                        .when(!suffix.is_empty(), move |this| {
                            this.child(crate::ui::search_highlight::render_highlighted_inline(
                                suffix,
                                &terms,
                                text_color,
                                highlight_bg,
                                highlight_text,
                                font_size,
                                font_weight,
                            ))
                        })
                        .into_any_element(),
                }
            }))
            .into_any_element()
    }
}
