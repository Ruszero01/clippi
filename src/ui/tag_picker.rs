//! --- Tag picker panel - assign or remove tags on clipboard items. ---

use std::rc::Rc;

type TagToggleHandler = Rc<dyn Fn(i64, TagState, &mut gpui::Window, &mut gpui::App)>;
type PanelHandler = Rc<dyn Fn(&mut gpui::Window, &mut gpui::App)>;
type CreateTagHandler = Rc<dyn Fn(String, &mut gpui::Window, &mut gpui::App)>;

use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::input::{Input, InputState};

use crate::core::i18n_keys::I18nKey;

use crate::core::types::TagInfo;

use super::theme::ClippiTheme;

#[derive(Clone, Copy, PartialEq)]
pub enum TagState {
    None,
    All,
    Partial,
}

#[derive(IntoElement)]
pub struct TagPickerPanel {
    tags: Vec<(TagInfo, TagState)>,
    is_batch: bool,
    create_input: Entity<InputState>,
    on_toggle: Option<TagToggleHandler>,
    on_clear: Option<PanelHandler>,
    on_close: Option<PanelHandler>,
    on_create: Option<CreateTagHandler>,
    theme: ClippiTheme,
}

impl TagPickerPanel {
    pub fn new(
        tags: Vec<(TagInfo, TagState)>,
        is_batch: bool,
        create_input: Entity<InputState>,
        theme: ClippiTheme,
    ) -> Self {
        Self {
            tags,
            is_batch,
            create_input,
            on_toggle: None,
            on_clear: None,
            on_close: None,
            on_create: None,
            theme,
        }
    }

    pub fn on_toggle(
        mut self,
        handler: impl Fn(i64, TagState, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }

    pub fn on_clear(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_clear = Some(Rc::new(handler));
        self
    }

    pub fn on_close(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(handler));
        self
    }

    pub fn on_create(
        mut self,
        handler: impl Fn(String, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_create = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for TagPickerPanel {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Self {
            tags,
            is_batch,
            create_input,
            on_toggle,
            on_clear,
            on_close,
            on_create,
            theme,
        } = self;

        let is_dark = theme.bg == rgb(0x191a1b);
        let surface = theme.panel_surface;
        let input_bg = theme.panel_input_bg;
        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;
        let accent = theme.accent;
        let btn_hover = theme.btn_hover;
        let accent_hover_bg = if is_dark {
            rgba(0x7ecba335)
        } else {
            rgba(0x6ab89035)
        };
        let sep_line = theme.panel_sep_line;
        let panel_border = if is_dark {
            rgba(0xffffff14)
        } else {
            rgba(0x00000012)
        };
        let active_bg = theme.accent_overlay();

        let rows: Vec<Vec<(TagInfo, TagState)>> = tags.chunks(2).map(|row| row.to_vec()).collect();
        let is_empty = rows.is_empty();

        div()
            .flex()
            .flex_col()
            .w(px(304.))
            .max_h(px(300.))
            .bg(surface)
            .border(px(1.))
            .border_color(panel_border)
            .rounded(px(8.))
            .shadow_lg()
            .p(px(8.))
            .gap(px(4.))
            .child(
                div()
                    .h(px(24.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_1)
                            .child(if is_batch { I18nKey::CtxBatchTag.text() } else { I18nKey::CtxTag.text() }),
                    )
                    .child(div().flex_1())
                    .child(icon_button("\u{e62e}", text_2, btn_hover, Some(I18nKey::TagTooltipClear.text()), on_clear))
                    .child(icon_button("\u{e7b7}", text_2, btn_hover, None, on_close)),
            )
            .child(div().h(px(1.)).w_full().bg(sep_line))
            // --- Create tag row ---
            .child({
                let on_create = on_create.clone();
                let input = create_input.clone();
                div()
                    .h(px(26.))
                    .flex()
                    .flex_row()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex_1()
                            .h(px(26.))
                            .rounded(px(5.))
                            .bg(input_bg)
                            .px(px(7.))
                            .flex()
                            .items_center()
                            .child(
                                Input::new(&create_input)
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .w_full()
                                    .h(px(20.))
                                    .text_size(px(11.))
                                    .text_color(text_1),
                            )
                            .on_key_down({
                                let on_create = on_create.clone();
                                let input = input.clone();
                                move |ev: &KeyDownEvent, window, cx| {
                                    if ev.keystroke.key.as_str() == "enter" {
                                        cx.stop_propagation();
                                        let name = input.read(cx).value().to_string();
                                        if !name.trim().is_empty() {
                                            if let Some(ref handler) = on_create {
                                                handler(name.trim().to_string(), window, cx);
                                            }
                                            input.update(cx, |input, cx| {
                                                input.set_value("", window, cx);
                                            });
                                        }
                                    }
                                }
                            }),
                    )
                    .child({
                        let on_create = on_create.clone();
                        let input = input.clone();
                        div()
                            .w(px(26.))
                            .h(px(26.))
                            .rounded(px(5.))
                            .bg(btn_hover)
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(move |style| style.bg(accent_hover_bg))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(accent)
                                    .child(I18nKey::TagFilterAdd.text()),
                            )
                            .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                let name = input.read(cx).value().to_string();
                                if !name.trim().is_empty() {
                                    if let Some(ref handler) = on_create {
                                        handler(name.trim().to_string(), window, cx);
                                    }
                                    input.update(cx, |input, cx| {
                                        input.set_value("", window, cx);
                                    });
                                }
                            })
                    })
            })
            .child(div().h(px(1.)).w_full().bg(sep_line))
            .when(is_empty, |el| {
                el.child(
                    div()
                        .px(px(6.))
                        .py(px(12.))
                        .text_size(px(11.))
                        .text_color(text_3)
                        .child(I18nKey::TagPickerNoTags.text()),
                )
            })
            .when(!is_empty, |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .max_h(px(230.))
                        .overflow_hidden()
                        .gap(px(4.))
                        .children(rows.into_iter().map(|row| {
                            let on_toggle = on_toggle.clone();
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(4.))
                                .children(row.into_iter().map(move |(tag, state)| {
                                    let cell_colors = TagCellColors {
                                        active_bg,
                                        input_bg,
                                        hover_bg: btn_hover,
                                        accent,
                                        text_1,
                                        text_2,
                                    };
                                    tag_cell(tag, state, on_toggle.clone(), &cell_colors)
                                }))
                        })),
                )
            })
    }
}

fn icon_button(
    icon: &'static str,
    color: Rgba,
    hover_bg: Rgba,
    tooltip: Option<&'static str>,
    handler: Option<PanelHandler>,
) -> Stateful<Div> {
    let button = div()
        .id(icon)
        .w(px(22.))
        .h(px(22.))
        .rounded(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::PointingHand)
        .hover(move |style| style.bg(hover_bg))
        .when_some(tooltip, |button, tip| {
            button.tooltip(move |window, cx| {
                Tooltip::element(move |_window, _cx| {
                    div().text_size(px(10.)).child(tip)
                })
                .build(window, cx)
            })
        })
        .child(
            div()
                .font_family("iconfont")
                .text_size(px(11.))
                .text_color(color)
                .child(icon),
        );

    if let Some(handler) = handler {
        button.on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            cx.stop_propagation();
            handler(window, cx);
        })
    } else {
        button
    }
}

struct TagCellColors {
    active_bg: Rgba,
    input_bg: Rgba,
    hover_bg: Rgba,
    accent: Rgba,
    text_1: Rgba,
    text_2: Rgba,
}

fn tag_cell(
    tag: TagInfo,
    state: TagState,
    on_toggle: Option<TagToggleHandler>,
    colors: &TagCellColors,
) -> Div {
    let tag_id = tag.id;
    let active = state != TagState::None;
    let tag_color = color_from_hex(&tag.color, colors.accent);
    let state_mark = match state {
        TagState::All => "*",
        TagState::Partial => "-",
        TagState::None => "",
    };

    let cell = div()
        .w(px(140.))
        .h(px(30.))
        .rounded(px(5.))
        .bg(if active {
            colors.active_bg
        } else {
            colors.input_bg
        })
        .px(px(6.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.))
        .cursor(CursorStyle::PointingHand)
        .hover(move |style| {
            style.bg(if active {
                colors.active_bg
            } else {
                colors.hover_bg
            })
        })
        .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(tag_color))
        .child(
            div()
                .flex_1()
                .text_size(px(11.))
                .font_weight(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if active { colors.accent } else { colors.text_1 })
                .truncate()
                .child(tag.name),
        )
        .child(
            div()
                .w(px(12.))
                .text_size(px(11.))
                .text_color(if active { colors.accent } else { colors.text_2 })
                .child(state_mark),
        );

    if let Some(handler) = on_toggle {
        cell.on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            cx.stop_propagation();
            handler(tag_id, state, window, cx);
        })
    } else {
        cell
    }
}

fn color_from_hex(hex: &str, fallback: Rgba) -> Rgba {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() == 6 {
        if let Ok(value) = u32::from_str_radix(hex, 16) {
            return rgb(value);
        }
    }
    fallback
}
