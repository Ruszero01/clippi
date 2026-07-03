//! Add/edit sync backend dialog.

use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_transitions::WindowUseTransition;

use crate::core::i18n_keys::I18nKey;
use crate::core::settings::{compose_webdav_url, BackendConfig};
use crate::services::backends::local_folder::detect_presets;
use crate::services::gpui_sync::test_webdav_connection;
use crate::ui::theme::ClippiTheme;
use crate::ui::window_manager::{WebDavBackendForm, WindowManager};

const BACKEND_PANEL_ANIM_DURATION: Duration = Duration::from_millis(150);
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditorStep {
    SelectType,
    LocalFolder,
    WebDav,
}

pub struct AddBackendPanel {
    visible: bool,
    animation_generation: u64,
    animation_started: Option<Instant>,
    edit_id: Option<String>,
    step: EditorStep,
    theme: ClippiTheme,
    window_manager: Entity<WindowManager>,
    presets: Vec<(&'static str, String)>,
    name_input: Entity<InputState>,
    folder_input: Entity<InputState>,
    url_input: Entity<InputState>,
    webdav_path_input: Entity<InputState>,
    username_input: Entity<InputState>,
    password_input: Entity<InputState>,
    test_pending: bool,
    test_ok: bool,
    test_error: String,
    _test_task: Option<Task<()>>,
    _folder_dialog_task: Option<Task<()>>,
    last_lang_version: u64,
}

impl AddBackendPanel {
    pub fn new(
        window_manager: Entity<WindowManager>,
        theme: ClippiTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            visible: false,
            animation_generation: 0,
            animation_started: None,
            edit_id: None,
            step: EditorStep::SelectType,
            theme,
            window_manager,
            presets: detect_presets(),
            name_input: cx
                .new(|cx| InputState::new(window, cx).placeholder(I18nKey::BackendPhName.text())),
            folder_input: cx
                .new(|cx| InputState::new(window, cx).placeholder(I18nKey::BackendPhFolder.text())),
            url_input: cx
                .new(|cx| InputState::new(window, cx).placeholder(I18nKey::BackendPhUrl.text())),
            webdav_path_input: cx.new(|cx| {
                InputState::new(window, cx).placeholder(I18nKey::BackendPhWebdavPath.text())
            }),
            username_input: cx
                .new(|cx| InputState::new(window, cx).placeholder(I18nKey::BackendPhUser.text())),
            password_input: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(I18nKey::BackendPhPass.text())
                    .masked(true)
            }),
            test_pending: false,
            test_ok: false,
            test_error: String::new(),
            _test_task: None,
            _folder_dialog_task: None,
            last_lang_version: crate::core::i18n::lang_version(),
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn set_theme(&mut self, theme: ClippiTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    pub fn open_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = true;
        self.animation_generation = self.animation_generation.wrapping_add(1);
        self.animation_started = Some(Instant::now());
        self.edit_id = None;
        self.step = EditorStep::SelectType;
        self.clear_inputs(window, cx);
        self.reset_test();
        cx.notify();
    }

    pub fn open_edit(
        &mut self,
        config: &BackendConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.visible = true;
        self.animation_generation = self.animation_generation.wrapping_add(1);
        self.animation_started = Some(Instant::now());
        self.edit_id = Some(config.id.clone());
        self.step = if config.backend_type == "webdav" {
            EditorStep::WebDav
        } else {
            EditorStep::LocalFolder
        };
        self.name_input
            .update(cx, |input, cx| input.set_value(&config.name, window, cx));
        self.folder_input.update(cx, |input, cx| {
            input.set_value(&config.folder_path, window, cx)
        });
        self.url_input.update(cx, |input, cx| {
            let root = if config.webdav_root_url.trim().is_empty() {
                config.webdav_url.clone()
            } else {
                config.webdav_root_url.clone()
            };
            input.set_value(root, window, cx)
        });
        self.webdav_path_input.update(cx, |input, cx| {
            input.set_value(config.webdav_path.clone(), window, cx)
        });
        self.username_input.update(cx, |input, cx| {
            input.set_value(&config.webdav_username, window, cx)
        });
        self.password_input.update(cx, |input, cx| {
            input.set_value(&config.webdav_password, window, cx)
        });
        self.reset_test();
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        self.animation_started = None;
        self.test_pending = false;
        self._test_task = None;
        cx.notify();
    }

    fn clear_inputs(&self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [
            &self.name_input,
            &self.folder_input,
            &self.url_input,
            &self.webdav_path_input,
            &self.username_input,
            &self.password_input,
        ] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    fn reset_test(&mut self) {
        self.test_pending = false;
        self.test_ok = false;
        self.test_error.clear();
    }

    fn transition_f32(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        key: (&'static str, u64),
        initial: f32,
        target: f32,
    ) -> f32 {
        let transition = window
            .use_keyed_transition(key, cx, BACKEND_PANEL_ANIM_DURATION, move |_, _| initial)
            .with_easing(Self::ease_out);
        transition.update(cx, |value, cx| {
            *value = target;
            cx.notify();
        });
        let value = *transition.evaluate(window, cx);
        value
    }

    fn ease_out(delta: f32) -> f32 {
        1.0 - (1.0 - delta).powi(3)
    }

    fn start_webdav_test(&mut self, cx: &mut Context<Self>) {
        let url = compose_webdav_url(
            self.url_input.read(cx).value(),
            self.webdav_path_input.read(cx).value(),
        );
        if url.trim().is_empty() || self.test_pending {
            return;
        }
        let username = self.username_input.read(cx).value().to_string();
        let password = self.password_input.read(cx).unmask_value().to_string();
        self.test_pending = true;
        self.test_ok = false;
        self.test_error.clear();

        let background =
            cx.background_spawn(async move { test_webdav_connection(&url, &username, &password) });
        self._test_task = Some(cx.spawn(async move |weak, cx| {
            let ok = background.await;
            if let Some(this) = weak.upgrade() {
                let _ = this.update(cx, |panel, cx| {
                    panel.test_pending = false;
                    panel.test_ok = ok;
                    panel.test_error = if ok {
                        String::new()
                    } else {
                        I18nKey::BackendTestFail.text().into()
                    };
                    cx.notify();
                });
            }
        }));
        cx.notify();
    }

    fn title(&self) -> &'static str {
        if self.edit_id.is_some() {
            I18nKey::BackendEditTitle.text()
        } else {
            match self.step {
                EditorStep::SelectType => I18nKey::BackendAddTitle.text(),
                EditorStep::LocalFolder => I18nKey::BackendLocalFolder.text(),
                EditorStep::WebDav => I18nKey::BackendWebdav.text(),
            }
        }
    }

    fn render_type_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().clone();
        let accent = self.theme.accent;
        let text_1 = self.theme.text_1;
        let text_3 = self.theme.text_3;
        let card_bg = if self.theme.bg == rgb(0x191a1b) {
            rgb(0x2a2b2c)
        } else {
            rgb(0xf0f1f5)
        };

        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(self.theme.text_2)
                    .child(I18nKey::BackendSelectType.text()),
            )
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .child(type_card(
                        I18nKey::BackendLocalFolder.text(),
                        I18nKey::BackendLocalDesc.text(),
                        accent,
                        card_bg,
                        text_1,
                        text_3,
                        {
                            let this = this.clone();
                            move |_window, cx| {
                                this.update(cx, |panel, cx| {
                                    panel.step = EditorStep::LocalFolder;
                                    cx.notify();
                                });
                            }
                        },
                    ))
                    .child(type_card(
                        I18nKey::BackendWebdav.text(),
                        I18nKey::BackendWebdavDesc.text(),
                        accent,
                        card_bg,
                        text_1,
                        text_3,
                        move |window, cx| {
                            this.update(cx, |panel, cx| {
                                panel.step = EditorStep::WebDav;
                                if panel.name_input.read(cx).value().is_empty() {
                                    panel.name_input.update(cx, |input, cx| {
                                        input.set_value(I18nKey::BackendWebdav.text(), window, cx)
                                    });
                                }
                                cx.notify();
                            });
                        },
                    )),
            )
            .into_any_element()
    }

    fn render_local_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let editing = self.edit_id.is_some();
        let accent = self.theme.accent;
        let accent_soft = self.theme.accent_soft;
        let divider = self.theme.divider;
        let text_2 = self.theme.text_2;
        let text_1 = self.theme.text_1;
        let card_bg = if self.theme.bg == rgb(0x191a1b) {
            rgb(0x2a2b2c)
        } else {
            rgb(0xf0f1f5)
        };
        let this = cx.entity().clone();

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .when(!editing && !self.presets.is_empty(), |form| {
                form.child(field_label(I18nKey::BackendQuickAdd.text(), text_2))
                    .child(div().flex().gap(px(8.)).children(self.presets.iter().map(
                        |(name, path)| {
                            let name = (*name).to_string();
                            let input_name = name.clone();
                            let path = path.clone();
                            let name_input = self.name_input.clone();
                            let folder_input = self.folder_input.clone();
                            div()
                                .h(px(34.))
                                .px(px(12.))
                                .rounded(px(7.))
                                .bg(card_bg)
                                .border(px(1.))
                                .border_color(divider)
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .cursor(CursorStyle::PointingHand)
                                .hover(move |style| style.border_color(accent))
                                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                    if name_input.read(cx).value().is_empty() {
                                        name_input.update(cx, |input, cx| {
                                            input.set_value(&input_name, window, cx)
                                        });
                                    }
                                    folder_input
                                        .update(cx, |input, cx| input.set_value(&path, window, cx));
                                })
                                .child({
                                    let icon = match name.as_str() {
                                        "OneDrive" => "\u{e601}",
                                        "iCloud" => "\u{ebc8}",
                                        _ => "\u{e60a}",
                                    };
                                    div()
                                        .font_family("iconfont")
                                        .text_size(px(14.))
                                        .text_color(text_2)
                                        .child(icon)
                                })
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(self.theme.text_1)
                                        .child(name.clone()),
                                )
                        },
                    )))
            })
            .when(!editing && !self.presets.is_empty(), |form| {
                form.child(div().h(px(1.)).bg(divider))
            })
            .child(field_label(I18nKey::BackendFolder.text(), text_2))
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_1()
                            .child(input_box(&self.folder_input, &self.theme)),
                    )
                    .child(
                        div()
                            .w(px(58.))
                            .h(px(30.))
                            .rounded(px(6.))
                            .bg(card_bg)
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_2)
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(move |style| style.bg(divider).text_color(text_1))
                            .on_mouse_down(MouseButton::Left, {
                                let folder_input = self.folder_input.clone();
                                let this = this.clone();
                                move |_ev, window, cx| {
                                    let input = folder_input.clone();
                                    let window_handle = window.window_handle();
                                    let dialog = rfd::AsyncFileDialog::new().pick_folder();
                                    let task = cx.spawn(async move |cx| {
                                        if let Some(path) = dialog.await {
                                            let path = path.path().to_string_lossy().to_string();
                                            let _ =
                                                cx.update_window(window_handle, |_, window, cx| {
                                                    input.update(cx, |input, cx| {
                                                        input.set_value(&path, window, cx);
                                                    });
                                                });
                                        }
                                    });
                                    this.update(cx, |panel, _cx| {
                                        panel._folder_dialog_task = Some(task);
                                    });
                                }
                            })
                            .child(I18nKey::BackendBrowse.text()),
                    ),
            )
            .child(div().h(px(1.)).bg(divider))
            .child(primary_button(
                if editing {
                    I18nKey::BackendSave.text()
                } else {
                    I18nKey::BackendAddTitle.text()
                },
                accent,
                accent_soft,
                {
                    let name_input = self.name_input.clone();
                    let folder_input = self.folder_input.clone();
                    let edit_id = self.edit_id.clone();
                    let wm = self.window_manager.clone();
                    move |window, cx| {
                        let name = name_input.read(cx).value().to_string();
                        if name.trim().is_empty() {
                            name_input.update(cx, |input, cx| input.focus_handle(cx).focus(window));
                            wm.update(cx, |wm, cx| {
                                wm.show_warning_toast(I18nKey::BackendNameRequired.text(), cx);
                            });
                            return;
                        }
                        let folder = folder_input.read(cx).value().to_string();
                        if folder.trim().is_empty() {
                            return;
                        }
                        wm.update(cx, |wm, cx| {
                            if let Some(id) = edit_id.as_deref() {
                                wm.edit_backend(id, name, folder, cx);
                            } else {
                                wm.add_local_folder_backend(name, folder, cx);
                            }
                        });
                        this.update(cx, |panel, cx| panel.close(cx));
                    }
                },
            ))
            .into_any_element()
    }

    fn render_webdav_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let editing = self.edit_id.is_some();
        let accent = self.theme.accent;
        let accent_soft = self.theme.accent_soft;
        let divider = self.theme.divider;
        let text_2 = self.theme.text_2;
        let _text_1 = self.theme.text_1;
        let _card_bg = if self.theme.bg == rgb(0x191a1b) {
            rgb(0x2a2b2c)
        } else {
            rgb(0xf0f1f5)
        };
        let this = cx.entity().clone();

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(field_label(I18nKey::BackendServerUrl.text(), text_2))
                    .child(input_box(&self.url_input, &self.theme)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(field_label(I18nKey::BackendWebdavPath.text(), text_2))
                    .child(input_box(&self.webdav_path_input, &self.theme)),
            )
            .child(
                div()
                    .flex()
                    .gap(px(7.))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(field_label(I18nKey::BackendUsername.text(), text_2))
                            .child(input_box(&self.username_input, &self.theme)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(field_label(I18nKey::BackendPassword.text(), text_2))
                            .child(input_box(&self.password_input, &self.theme)),
                    ),
            )
            .child(div().h(px(1.)).bg(divider))
            .when(!self.test_error.is_empty(), |form| {
                form.child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(self.theme.danger)
                        .child(self.test_error.clone()),
                )
            })
            .when(!self.test_ok, |form| {
                form.child(primary_button(
                    if self.test_pending {
                        I18nKey::BackendTesting.text()
                    } else {
                        I18nKey::BackendTest.text()
                    },
                    accent,
                    accent_soft,
                    {
                        let this = this.clone();
                        move |_window, cx| {
                            this.update(cx, |panel, cx| {
                                panel.start_webdav_test(cx);
                            });
                        }
                    },
                ))
            })
            .when(self.test_ok, |form| {
                form.child(primary_button(
                    if editing {
                        I18nKey::BackendSave.text()
                    } else {
                        I18nKey::BackendAddTitle.text()
                    },
                    rgb(0x4caf50),
                    accent_soft,
                    {
                        let name_input = self.name_input.clone();
                        let url_input = self.url_input.clone();
                        let path_input = self.webdav_path_input.clone();
                        let username_input = self.username_input.clone();
                        let password_input = self.password_input.clone();
                        let edit_id = self.edit_id.clone();
                        let wm = self.window_manager.clone();
                        move |window, cx| {
                            let name = name_input.read(cx).value().to_string();
                            if name.trim().is_empty() {
                                name_input
                                    .update(cx, |input, cx| input.focus_handle(cx).focus(window));
                                wm.update(cx, |wm, cx| {
                                    wm.show_warning_toast(I18nKey::BackendNameRequired.text(), cx);
                                });
                                return;
                            }
                            let root_url = url_input.read(cx).value().to_string();
                            let path = path_input.read(cx).value().to_string();
                            let url = compose_webdav_url(&root_url, &path);
                            if url.trim().is_empty() {
                                return;
                            }
                            let username = username_input.read(cx).value().to_string();
                            let password = password_input.read(cx).unmask_value().to_string();
                            wm.update(cx, |wm, cx| {
                                let form = WebDavBackendForm {
                                    name,
                                    root_url,
                                    path,
                                    username,
                                    password,
                                };
                                if let Some(id) = edit_id.as_deref() {
                                    wm.edit_webdav_backend(id, form, cx);
                                } else {
                                    wm.add_webdav_backend(form, cx);
                                }
                            });
                            this.update(cx, |panel, cx| panel.close(cx));
                        }
                    },
                ))
            })
            .into_any_element()
    }
}

impl Render for AddBackendPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 语言切换时刷新 InputState placeholder
        let current = crate::core::i18n::lang_version();
        if self.last_lang_version != current {
            self.last_lang_version = current;
            self.name_input.update(cx, |state, cx| {
                state.set_placeholder(I18nKey::BackendPhName.text(), window, cx);
            });
            self.folder_input.update(cx, |state, cx| {
                state.set_placeholder(I18nKey::BackendPhFolder.text(), window, cx);
            });
            self.url_input.update(cx, |state, cx| {
                state.set_placeholder(I18nKey::BackendPhUrl.text(), window, cx);
            });
            self.webdav_path_input.update(cx, |state, cx| {
                state.set_placeholder(I18nKey::BackendPhWebdavPath.text(), window, cx);
            });
            self.username_input.update(cx, |state, cx| {
                state.set_placeholder(I18nKey::BackendPhUser.text(), window, cx);
            });
            self.password_input.update(cx, |state, cx| {
                state.set_placeholder(I18nKey::BackendPhPass.text(), window, cx);
            });
        }

        if !self.visible {
            return div().into_any_element();
        }

        let this = cx.entity().clone();
        let show_back = self.edit_id.is_none() && self.step != EditorStep::SelectType;
        let content = match self.step {
            EditorStep::SelectType => self.render_type_picker(cx),
            EditorStep::LocalFolder => self.render_local_form(cx),
            EditorStep::WebDav => self.render_webdav_form(cx),
        };
        let scale = if self.animation_started.is_some_and(|started| {
            started.elapsed() <= BACKEND_PANEL_ANIM_DURATION + Duration::from_millis(24)
        }) {
            self.transition_f32(
                window,
                cx,
                ("backend-panel-scale", self.animation_generation),
                0.96,
                1.0,
            )
        } else {
            1.0
        };

        div()
            .absolute()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, {
                let this = this.clone();
                move |_ev, _window, cx| {
                    cx.stop_propagation();
                    this.update(cx, |panel, cx| panel.close(cx));
                }
            })
            .child(
                div()
                    .w(px(304. * scale))
                    .max_h(px(440. * scale))
                    .overflow_hidden()
                    .rounded(px(8.))
                    .bg(self.theme.surface)
                    .border(px(1.))
                    .border_color(self.theme.divider)
                    .shadow_lg()
                    .p(px(12. * scale))
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
                        cx.stop_propagation();
                    })
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .items_center()
                                    .gap(px(6.))
                                    .when(show_back, |header| {
                                        header.child(
                                            div()
                                                .w(px(26.))
                                                .h(px(26.))
                                                .rounded(px(6.))
                                                .font_family("iconfont")
                                                .text_size(px(14.))
                                                .text_color(self.theme.text_2)
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .cursor(CursorStyle::PointingHand)
                                                .hover(|style| style.bg(rgba(0xffffff0d)))
                                                .on_mouse_down(MouseButton::Left, {
                                                    let this = this.clone();
                                                    move |_ev, _window, cx| {
                                                        cx.stop_propagation();
                                                        this.update(cx, |panel, cx| {
                                                            panel.step = EditorStep::SelectType;
                                                            panel.reset_test();
                                                            cx.notify();
                                                        });
                                                    }
                                                })
                                                .child("\u{e62b}"),
                                        )
                                    })
                                    .child(if self.step == EditorStep::SelectType {
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(self.theme.text_1)
                                            .child(self.title())
                                            .into_any_element()
                                    } else {
                                        header_name_input(&self.name_input, &self.theme)
                                    }),
                            )
                            .child(
                                div()
                                    .w(px(26.))
                                    .h(px(26.))
                                    .rounded(px(6.))
                                    .font_family("iconfont")
                                    .text_size(px(13.))
                                    .text_color(self.theme.text_2)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(|style| style.bg(rgba(0xffffff0d)))
                                    .on_mouse_down(MouseButton::Left, {
                                        let this = this.clone();
                                        move |_ev, _window, cx| {
                                            cx.stop_propagation();
                                            this.update(cx, |panel, cx| panel.close(cx));
                                        }
                                    })
                                    .child("\u{e7b7}"),
                            ),
                    )
                    .child(div().h(px(1.)).bg(self.theme.divider))
                    .child(div().max_h(px(380.)).overflow_y_scrollbar().child(content)),
            )
            .into_any_element()
    }
}

fn header_name_input(input: &Entity<InputState>, theme: &ClippiTheme) -> AnyElement {
    div()
        .flex_1()
        .h(px(26.))
        .flex()
        .items_center()
        .child(
            Input::new(input)
                .appearance(false)
                .bordered(false)
                .focus_bordered(false)
                .w_full()
                .h(px(20.))
                .text_size(px(13.))
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_1),
        )
        .into_any_element()
}

fn field_label(label: &'static str, color: Rgba) -> impl IntoElement {
    div()
        .text_size(px(11.))
        .font_weight(FontWeight::BOLD)
        .text_color(color)
        .child(label)
}

fn input_box(input: &Entity<InputState>, theme: &ClippiTheme) -> AnyElement {
    div()
        .h(px(30.))
        .rounded(px(6.))
        .bg(if theme.bg == rgb(0x191a1b) {
            rgb(0x191a1b)
        } else {
            rgb(0xf2f3f8)
        })
        .px(px(7.))
        .flex()
        .items_center()
        .child(
            Input::new(input)
                .appearance(false)
                .bordered(false)
                .focus_bordered(false)
                .w_full()
                .h(px(22.))
                .text_size(px(11.))
                .text_color(theme.text_1),
        )
        .into_any_element()
}

fn type_card(
    title: &'static str,
    description: &'static str,
    accent: Rgba,
    background: Rgba,
    text_1: Rgba,
    text_3: Rgba,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex_1()
        .h(px(78.))
        .rounded(px(8.))
        .bg(background)
        .border(px(1.))
        .border_color(rgba(0x00000000))
        .p(px(10.))
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(4.))
        .cursor(CursorStyle::PointingHand)
        .hover(move |style| style.border_color(accent))
        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            on_click(window, cx);
        })
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::BOLD)
                .text_color(text_1)
                .child(title),
        )
        .child(
            div()
                .text_size(px(10.))
                .text_color(text_3)
                .child(description),
        )
}

fn primary_button(
    label: &'static str,
    color: Rgba,
    hover: Rgba,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .h(px(34.))
        .rounded(px(7.))
        .bg(color)
        .text_size(px(12.))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(0xffffff))
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::PointingHand)
        .hover(move |style| style.bg(hover).text_color(color))
        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            on_click(window, cx);
        })
        .child(label)
}
