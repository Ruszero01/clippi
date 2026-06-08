//! Add/edit sync backend dialog.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement;

use crate::core::settings::BackendConfig;
use crate::services::backends::local_folder::detect_presets;
use crate::services::gpui_sync::test_webdav_connection;
use crate::ui::theme::ClippiTheme;
use crate::ui::window_manager::WindowManager;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditorStep {
    SelectType,
    LocalFolder,
    WebDav,
}

pub struct AddBackendPanel {
    visible: bool,
    edit_id: Option<String>,
    step: EditorStep,
    theme: ClippiTheme,
    window_manager: Entity<WindowManager>,
    presets: Vec<(&'static str, String)>,
    name_input: Entity<InputState>,
    folder_input: Entity<InputState>,
    url_input: Entity<InputState>,
    username_input: Entity<InputState>,
    password_input: Entity<InputState>,
    test_pending: bool,
    test_ok: bool,
    test_error: String,
    _test_task: Option<Task<()>>,
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
            edit_id: None,
            step: EditorStep::SelectType,
            theme,
            window_manager,
            presets: detect_presets(),
            name_input: cx.new(|cx| InputState::new(window, cx).placeholder("Backend name")),
            folder_input: cx.new(|cx| InputState::new(window, cx).placeholder("Folder path")),
            url_input: cx
                .new(|cx| InputState::new(window, cx).placeholder("https://example.com/dav")),
            username_input: cx.new(|cx| InputState::new(window, cx).placeholder("Username")),
            password_input: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Password")
                    .masked(true)
            }),
            test_pending: false,
            test_ok: false,
            test_error: String::new(),
            _test_task: None,
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
            input.set_value(&config.webdav_url, window, cx)
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

    fn close(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        self.test_pending = false;
        self._test_task = None;
        cx.notify();
    }

    fn clear_inputs(&self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [
            &self.name_input,
            &self.folder_input,
            &self.url_input,
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

    fn start_webdav_test(&mut self, cx: &mut Context<Self>) {
        let url = self.url_input.read(cx).value().to_string();
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
                        "Connection failed. Check URL and credentials.".into()
                    };
                    cx.notify();
                });
            }
        }));
        cx.notify();
    }

    fn title(&self) -> &'static str {
        if self.edit_id.is_some() {
            "Edit backend"
        } else {
            match self.step {
                EditorStep::SelectType => "Add backend",
                EditorStep::LocalFolder => "Local Folder",
                EditorStep::WebDav => "WebDAV",
            }
        }
    }

    fn render_type_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().clone();
        let accent = self.theme.accent;
        let accent_soft = self.theme.accent_soft;
        let text_1 = self.theme.text_1;
        let text_3 = self.theme.text_3;

        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(self.theme.text_2)
                    .child("Select backend type"),
            )
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .child(type_card(
                        "\u{e60a}",
                        "Local Folder",
                        "OneDrive, iCloud, etc.",
                        accent,
                        accent_soft,
                        text_1,
                        text_3,
                        {
                            let this = this.clone();
                            move |_window, cx| {
                                let _ = this.update(cx, |panel, cx| {
                                    panel.step = EditorStep::LocalFolder;
                                    cx.notify();
                                });
                            }
                        },
                    ))
                    .child(type_card(
                        "\u{e7b1}",
                        "WebDAV",
                        "NAS, Nextcloud, etc.",
                        accent,
                        accent_soft,
                        text_1,
                        text_3,
                        move |window, cx| {
                            let _ = this.update(cx, |panel, cx| {
                                panel.step = EditorStep::WebDav;
                                if panel.name_input.read(cx).value().is_empty() {
                                    panel.name_input.update(cx, |input, cx| {
                                        input.set_value("WebDAV", window, cx)
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
        let this = cx.entity().clone();

        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .when(!editing && !self.presets.is_empty(), |form| {
                form.child(field_label("Quick add", text_2)).child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .children(self.presets.iter().map(|(name, path)| {
                            let name = (*name).to_string();
                            let input_name = name.clone();
                            let path = path.clone();
                            let name_input = self.name_input.clone();
                            let folder_input = self.folder_input.clone();
                            div()
                                .h(px(34.))
                                .px(px(12.))
                                .rounded(px(7.))
                                .bg(accent_soft)
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
                                .child(
                                    div()
                                        .font_family("iconfont")
                                        .text_size(px(14.))
                                        .text_color(accent)
                                        .child("\u{e60a}"),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(self.theme.text_1)
                                        .child(name.clone()),
                                )
                        })),
                )
            })
            .when(!editing, |form| form.child(div().h(px(1.)).bg(divider)))
            .child(field_label("Name", text_2))
            .child(input_box(&self.name_input, &self.theme, false))
            .child(field_label("Folder", text_2))
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_1()
                            .child(input_box(&self.folder_input, &self.theme, false)),
                    )
                    .child(
                        div()
                            .w(px(58.))
                            .h(px(30.))
                            .rounded(px(6.))
                            .bg(accent_soft)
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(accent)
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(move |style| style.bg(accent).text_color(rgb(0xffffff)))
                            .on_mouse_down(MouseButton::Left, {
                                let folder_input = self.folder_input.clone();
                                move |_ev, window, cx| {
                                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                        folder_input.update(cx, |input, cx| {
                                            input.set_value(
                                                path.to_string_lossy().to_string(),
                                                window,
                                                cx,
                                            )
                                        });
                                    }
                                }
                            })
                            .child("Browse"),
                    ),
            )
            .child(primary_button(
                if editing { "Save" } else { "Add backend" },
                accent,
                accent_soft,
                {
                    let name_input = self.name_input.clone();
                    let folder_input = self.folder_input.clone();
                    let edit_id = self.edit_id.clone();
                    let wm = self.window_manager.clone();
                    move |_window, cx| {
                        let name = name_input.read(cx).value().to_string();
                        let folder = folder_input.read(cx).value().to_string();
                        if name.trim().is_empty() || folder.trim().is_empty() {
                            return;
                        }
                        wm.update(cx, |wm, cx| {
                            if let Some(id) = edit_id.as_deref() {
                                wm.edit_backend(id, name, folder, cx);
                            } else {
                                wm.add_local_folder_backend(name, folder, cx);
                            }
                        });
                        let _ = this.update(cx, |panel, cx| panel.close(cx));
                    }
                },
            ))
            .into_any_element()
    }

    fn render_webdav_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let editing = self.edit_id.is_some();
        let accent = self.theme.accent;
        let accent_soft = self.theme.accent_soft;
        let text_2 = self.theme.text_2;
        let this = cx.entity().clone();

        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(field_label("Server URL", text_2))
            .child(input_box(&self.url_input, &self.theme, false))
            .child(field_label("Name", text_2))
            .child(input_box(&self.name_input, &self.theme, false))
            .child(field_label("Username", text_2))
            .child(input_box(&self.username_input, &self.theme, false))
            .child(field_label("Password", text_2))
            .child(input_box(&self.password_input, &self.theme, true))
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
                        "Testing..."
                    } else {
                        "Test connection"
                    },
                    accent,
                    accent_soft,
                    {
                        let this = this.clone();
                        move |_window, cx| {
                            let _ = this.update(cx, |panel, cx| {
                                panel.start_webdav_test(cx);
                            });
                        }
                    },
                ))
            })
            .when(self.test_ok, |form| {
                form.child(primary_button(
                    if editing { "Save" } else { "Add backend" },
                    rgb(0x4caf50),
                    accent_soft,
                    {
                        let name_input = self.name_input.clone();
                        let url_input = self.url_input.clone();
                        let username_input = self.username_input.clone();
                        let password_input = self.password_input.clone();
                        let edit_id = self.edit_id.clone();
                        let wm = self.window_manager.clone();
                        move |_window, cx| {
                            let name = name_input.read(cx).value().to_string();
                            let url = url_input.read(cx).value().to_string();
                            if name.trim().is_empty() || url.trim().is_empty() {
                                return;
                            }
                            let username = username_input.read(cx).value().to_string();
                            let password = password_input.read(cx).unmask_value().to_string();
                            wm.update(cx, |wm, cx| {
                                if let Some(id) = edit_id.as_deref() {
                                    wm.edit_webdav_backend(id, name, url, username, password, cx);
                                } else {
                                    wm.add_webdav_backend(name, url, username, password, cx);
                                }
                            });
                            let _ = this.update(cx, |panel, cx| panel.close(cx));
                        }
                    },
                ))
            })
            .into_any_element()
    }
}

impl Render for AddBackendPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    let _ = this.update(cx, |panel, cx| panel.close(cx));
                }
            })
            .child(
                div()
                    .w(px(304.))
                    .max_h(px(440.))
                    .overflow_hidden()
                    .rounded(px(8.))
                    .bg(self.theme.surface)
                    .border(px(1.))
                    .border_color(self.theme.divider)
                    .shadow_lg()
                    .p(px(12.))
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
                        cx.stop_propagation();
                    })
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .when(show_back, |header| {
                                        header.child(
                                            div()
                                                .w(px(22.))
                                                .h(px(22.))
                                                .rounded(px(5.))
                                                .font_family("iconfont")
                                                .text_size(px(12.))
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
                                                        let _ = this.update(cx, |panel, cx| {
                                                            panel.step = EditorStep::SelectType;
                                                            panel.reset_test();
                                                            cx.notify();
                                                        });
                                                    }
                                                })
                                                .child("\u{e62b}"),
                                        )
                                    })
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(self.theme.text_1)
                                            .child(self.title()),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(22.))
                                    .h(px(22.))
                                    .rounded(px(5.))
                                    .font_family("iconfont")
                                    .text_size(px(11.))
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
                                            let _ = this.update(cx, |panel, cx| panel.close(cx));
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

fn field_label(label: &'static str, color: Rgba) -> impl IntoElement {
    div()
        .text_size(px(11.))
        .font_weight(FontWeight::BOLD)
        .text_color(color)
        .child(label)
}

fn input_box(input: &Entity<InputState>, theme: &ClippiTheme, password: bool) -> AnyElement {
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
                .when(password, |input| input.mask_toggle())
                .w_full()
                .h(px(22.))
                .text_size(px(11.))
                .text_color(theme.text_1),
        )
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn type_card(
    icon: &'static str,
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
        .gap(px(4.))
        .cursor(CursorStyle::PointingHand)
        .hover(move |style| style.border_color(accent))
        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            on_click(window, cx);
        })
        .child(
            div()
                .font_family("iconfont")
                .text_size(px(16.))
                .text_color(accent)
                .child(icon),
        )
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
