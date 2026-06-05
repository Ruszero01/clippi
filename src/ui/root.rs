//! Root view — the main window's top-level component.
//!
//! Matches the original Slint `app.slint` layout:
//! - Transparent window background
//! - Sidebar at x=0, y=84px in the transparent margin
//! - Main panel offset 36px from left, 12px border-radius, 1px border
//! - Titlebar + stacked views (clipboard / settings / edit)

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::state::app::AppState;
use crate::ui::window_manager::{WindowManager, WindowManagerEvent};

use super::clipboard_list::{ClipboardListView, ConfirmDialogState};
use super::components::confirm_dialog::ConfirmDialog;
use super::context_menu::{ContextMenu, MenuItemContext};
use super::search_bar::SearchBar;
use super::settings::{SettingsEvent, SettingsPanel};
use super::sidebar::Sidebar;
use super::tag_filter::{render_edit_panel, TagFilterPanel};
use super::theme::ClippiTheme;
use super::titlebar::{Titlebar, TitlebarEvent};

pub struct RootView {
    state: Entity<AppState>,
    window_manager: Entity<WindowManager>,
    titlebar: Entity<Titlebar>,
    list_view: Entity<ClipboardListView>,
    search_bar: Entity<SearchBar>,
    settings_panel: Entity<SettingsPanel>,
    sidebar: Entity<Sidebar>,
    tag_filter_panel: Entity<TagFilterPanel>,
    current_view: String,
    pinned: bool,
    theme: ClippiTheme,
    /// Cached at creation time so the ThemeChanged handler (which only
    /// has access to `&mut App`) can resolve the "system" theme correctly.
    window_appearance: WindowAppearance,
    last_edit_tag_id: i64,
    _wm_subscription: Subscription,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    pub fn new(
        window: &mut Window,
        state: Entity<AppState>,
        window_manager: Entity<WindowManager>,
        cx: &mut Context<Self>,
    ) -> Self {
        let app_state = state.read(cx);
        let items = app_state.items.clone();
        let window_appearance = window.appearance();
        let theme = ClippiTheme::from_setting(&app_state.settings.theme, Some(window_appearance));
        let list_view =
            cx.new(|cx| ClipboardListView::new(items, state.clone(), theme.clone(), window, cx));
        list_view.update(cx, |list, _cx| list.focus(window));
        let titlebar = cx.new(|_cx| Titlebar::new(state.clone(), list_view.clone(), theme.clone()));
        let search_bar = cx
            .new(|cx| SearchBar::new(state.clone(), list_view.clone(), theme.clone(), window, cx));
        let settings_panel =
            cx.new(|cx| SettingsPanel::new(state.clone(), window_manager.clone(), theme.clone(), cx));
        let sidebar = cx
            .new(|_cx| Sidebar::new(state.clone(), list_view.clone(), &theme));
        let tag_filter_panel = cx.new(|cx| {
            TagFilterPanel::new(
                state.clone(),
                list_view.clone(),
                search_bar.clone(),
                window,
                cx,
            )
        });

        // Subscribe to WindowManager events for clipboard changes and pin state.
        let _wm_subscription = cx.subscribe(
            &window_manager,
            move |this, _wm, event: &WindowManagerEvent, cx| match event {
                WindowManagerEvent::ClipboardChanged => {
                    let items = this.state.read(cx).items.clone();
                    this.list_view
                        .update(cx, |list, cx| list.set_items(items, cx));
                    cx.notify();
                }
                WindowManagerEvent::PinnedChanged(pinned) => {
                    this.pinned = *pinned;
                    this.titlebar
                        .update(cx, |tb, cx| tb.set_pinned(*pinned, cx));
                    cx.notify();
                }
                WindowManagerEvent::OpenSettings => {
                    this.current_view = "settings".into();
                    this.search_bar
                        .update(cx, |bar, cx| bar.close_tag_panel(cx));
                    cx.notify();
                }
            },
        );

        let wm = window_manager.clone();
        let titlebar_for_events = titlebar.clone();
        let _subscriptions = vec![
            cx.observe(&search_bar, |_this, _, cx| {
                cx.notify();
            }),
            cx.observe(&tag_filter_panel, |_this, _, cx| {
                cx.notify();
            }),
            cx.subscribe(
                &titlebar,
                move |this, _, event: &TitlebarEvent, cx| match event {
                    TitlebarEvent::TogglePin => {
                        this.pinned = !this.pinned;
                        let pinned = this.pinned;
                        wm.update(cx, |wm, cx| wm.set_pinned(pinned, cx));
                        titlebar_for_events.update(cx, |titlebar, cx| {
                            titlebar.set_pinned(pinned, cx);
                        });
                        cx.notify();
                    }
                    TitlebarEvent::OpenSettings => {
                        this.current_view = "settings".into();
                        this.search_bar
                            .update(cx, |bar, cx| bar.close_tag_panel(cx));
                        cx.notify();
                    }
                },
            ),
            cx.subscribe(
                &settings_panel,
                move |this, _panel, event: &SettingsEvent, cx| match event {
                    SettingsEvent::Back => {
                        this.current_view = "clipboard".into();
                        cx.notify();
                    }
                    SettingsEvent::ThemeChanged(theme_str) => {
                        // Use cached window_appearance from creation time so
                        // "system" theme resolves correctly even though we
                        // only have &mut App here (not WindowContext).
                        this.theme =
                            ClippiTheme::from_setting(theme_str, Some(this.window_appearance));
                        let theme = this.theme.clone();

                        // Sync gpui_component theme so that Input, Scrollbar
                        // and other gpui_component widgets follow our theme.
                        let is_dark = theme.bg == rgb(0x191a1b);
                        gpui_component::Theme::change(
                            if is_dark {
                                gpui_component::ThemeMode::Dark
                            } else {
                                gpui_component::ThemeMode::Light
                            },
                            None,
                            cx,
                        );
                        // Must restore transparent background after Theme::change
                        // resets it — otherwise the window loses transparency.
                        gpui_component::Theme::global_mut(cx).background =
                            Hsla::transparent_black();

                        let _ = this.titlebar.update(cx, |titlebar, cx| {
                            titlebar.set_theme(theme.clone(), cx);
                        });
                        let _ = this.search_bar.update(cx, |search_bar, cx| {
                            search_bar.set_theme(theme.clone(), cx);
                        });
                        let _ = this.list_view.update(cx, |list_view, cx| {
                            list_view.set_theme(theme.clone(), cx);
                        });
                        let _ = this.settings_panel.update(cx, |panel, cx| {
                            panel.reload_theme(theme.clone(), cx);
                        });
                        let _ = this.sidebar.update(cx, |sidebar, cx| {
                            sidebar.set_theme(&theme, cx);
                        });
                        cx.notify();
                    }
                },
            ),
        ];
        Self {
            state,
            window_manager,
            titlebar,
            list_view,
            search_bar,
            settings_panel,
            sidebar,
            tag_filter_panel,
            current_view: "clipboard".into(),
            pinned: false,
            theme,
            window_appearance,
            last_edit_tag_id: -1,
            _wm_subscription,
            _subscriptions,
        }
    }

    pub fn set_view(&mut self, view: &str, cx: &mut Context<Self>) {
        self.current_view = view.to_string();
        cx.notify();
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = self.sidebar.clone();
        let titlebar = self.titlebar.clone();
        let list_view = self.list_view.clone();
        let search_bar = self.search_bar.clone();
        let settings_panel = self.settings_panel.clone();
        let tag_filter_panel = self.tag_filter_panel.clone();
        let tag_panel_open = self.search_bar.read(cx).tag_panel_open();
        let is_clipboard = self.current_view == "clipboard";
        let theme = &self.theme;
        let panel_border = if theme.bg == rgb(0x191a1b) {
            rgb(0x3a3b3c)
        } else {
            rgb(0xd0d2de)
        };

        // Actual window dimensions for positioning overlays
        let viewport = window.viewport_size();
        let win_w = f32::from(viewport.width);
        let win_h = f32::from(viewport.height);

        div()
            .relative()
            .size_full()
            .child(div().absolute().left(px(0.)).top(px(84.)).child(sidebar))
            .child(
                div()
                    .absolute()
                    .left(px(36.))
                    .right(px(0.))
                    .top(px(0.))
                    .bottom(px(0.))
                    .rounded(px(12.))
                    .bg(theme.bg)
                    .border(px(1.))
                    .border_color(panel_border)
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .occlude()
                    .child(titlebar)
                    .when(is_clipboard, |panel| {
                        panel.child(search_bar.clone()).child(list_view.clone())
                    })
                    .when(!is_clipboard, |panel| panel.child(settings_panel)),
            )
            // Tag filter panel — ConfirmDialog pattern:
            // full-screen backdrop that closes on click outside,
            // panel positioned top-right, occlude prevents click-through.
            .when(tag_panel_open && is_clipboard, |root| {
                let search_for_backdrop = search_bar.clone();
                root.child(
                    div()
                        .absolute()
                        .size_full()
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            cx.stop_propagation();
                            search_for_backdrop.update(cx, |bar, cx| bar.close_tag_panel(cx));
                        })
                        .child(
                            div()
                                .absolute()
                                .right(px(8.))
                                .top(px(106.))
                                .occlude()
                                .child(tag_filter_panel),
                        ),
                )
            })
            // Tag edit overlay — centered in main panel area (left:36px)
            .when(
                {
                    let app_state = self.state.read(cx);
                    let editing_id = app_state.editing_tag_id;
                    if editing_id >= 0 && editing_id != self.last_edit_tag_id {
                        self.last_edit_tag_id = editing_id;
                        let edit_name = app_state.editing_tag_name.clone();
                        let edit_input =
                            self.tag_filter_panel.read(cx).edit_name_input().clone();
                        let _ = edit_input.update(cx, |input, cx| {
                            input.set_value(&edit_name, window, cx);
                        });
                    }
                    editing_id >= 0 && is_clipboard
                },
                |root| {
                    let app_state = self.state.read(cx);
                    let editing_tag_id = app_state.editing_tag_id;
                    let editing_tag_color = app_state.editing_tag_color.clone();
                    let edit_name_input =
                        self.tag_filter_panel.read(cx).edit_name_input().clone();
                    let tag_filter = self.tag_filter_panel.clone();

                    root.child(
                        div()
                            .absolute()
                            .left(px(36.))
                            .right(px(0.))
                            .top(px(0.))
                            .bottom(px(0.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(render_edit_panel(
                                &edit_name_input,
                                &editing_tag_color,
                                self.theme.clone(),
                                {
                                    let tf = tag_filter.clone();
                                    move |_w, cx| {
                                        let _ = tf.update(cx, |panel, cx| {
                                            panel.cancel_edit_tag(cx);
                                            cx.notify();
                                        });
                                    }
                                },
                                {
                                    let tf = tag_filter.clone();
                                    move |_w, cx, color| {
                                        let _ = tf.update(cx, |panel, cx| {
                                            panel.set_edit_tag_color(&color, cx);
                                            cx.notify();
                                        });
                                    }
                                },
                                {
                                    let tf = tag_filter.clone();
                                    move |_w, cx, name, color| {
                                        let _ = tf.update(cx, |panel, cx| {
                                            panel.update_tag(editing_tag_id, &name, &color, cx);
                                            cx.notify();
                                        });
                                    }
                                },
                            )),
                    )
                },
            )
            .when(
                self.list_view.read(cx).context_menu_visible() && is_clipboard,
                |root| {
                    let list = self.list_view.clone();
                    let list_for_action = self.list_view.clone();
                    let (menu_x, menu_y) = list.read(cx).context_menu_position();
                    let is_batch = list.read(cx).context_menu_is_batch();
                    let item = list.read(cx).context_menu_item().cloned();

                    // Backdrop — click to dismiss
                    root.child(
                        div()
                            .absolute()
                            .size_full()
                            .on_mouse_down(MouseButton::Left, {
                                let l = list.clone();
                                move |_ev, _window, cx| {
                                    cx.stop_propagation();
                                    let _ = l.update(cx, |lst, cx| lst.dismiss_context_menu(cx));
                                }
                            }),
                    )
                    .child(div().absolute().occlude().child({
                        let l = list_for_action.clone();
                        if is_batch {
                            let count = l.read(cx).selected_count;
                            ContextMenu::for_batch(count)
                                .with_position(menu_x, menu_y, win_w, win_h)
                                .on_action({
                                    let l = l.clone();
                                    move |action, window, cx| {
                                        let _ = l.update(cx, |lst, cx| {
                                            lst.handle_menu_action(action, window, cx);
                                        });
                                    }
                                })
                                .on_dismiss({
                                    let l = l.clone();
                                    move |_window, cx| {
                                        let _ = l.update(cx, |lst, cx| {
                                            lst.hide_context_menu(cx);
                                        });
                                    }
                                })
                                .into_any_element()
                        } else if let Some(ref clip_item) = item {
                            let ctx = MenuItemContext::from_item(clip_item);
                            ContextMenu::for_item(&ctx)
                                .with_position(menu_x, menu_y, win_w, win_h)
                                .on_action({
                                    let l = l.clone();
                                    move |action, window, cx| {
                                        let _ = l.update(cx, |lst, cx| {
                                            lst.handle_menu_action(action, window, cx);
                                        });
                                    }
                                })
                                .on_dismiss({
                                    let l = l.clone();
                                    move |_window, cx| {
                                        let _ = l.update(cx, |lst, cx| {
                                            lst.hide_context_menu(cx);
                                        });
                                    }
                                })
                                .into_any_element()
                        } else {
                            div().into_any_element()
                        }
                    }))
                },
            )
            .when(
                self.list_view.read(cx).confirm_dialog_state().is_some() && is_clipboard,
                |root| {
                    let list = self.list_view.clone();
                    let app_state = self.state.clone();

                    // Read dialog state and clone what we need before closures
                    let dialog = list.read(cx).confirm_dialog_state().cloned();
                    let dialog_element: AnyElement = match dialog {
                        Some(ConfirmDialogState::DeleteSingle { id }) => {
                            ConfirmDialog::delete_single()
                                .on_confirm({
                                    let s = app_state.clone();
                                    let l = list.clone();
                                    move |_window, cx| {
                                        s.update(cx, |s, _cx| s.delete_item(id));
                                        l.update(cx, |lst, cx| {
                                            lst.sync_items_from_state(cx);
                                            lst.dismiss_confirm_dialog(cx);
                                        });
                                    }
                                })
                                .on_cancel({
                                    let l = list.clone();
                                    move |_window, cx| {
                                        l.update(cx, |lst, cx| lst.dismiss_confirm_dialog(cx));
                                    }
                                })
                                .into_any_element()
                        }
                        Some(ConfirmDialogState::DeleteBatch { count }) => {
                            ConfirmDialog::delete_batch(count)
                                .on_confirm({
                                    let s = app_state.clone();
                                    let l = list.clone();
                                    move |_window, cx| {
                                        s.update(cx, |s, _cx| s.batch_delete());
                                        l.update(cx, |lst, cx| {
                                            lst.sync_items_from_state(cx);
                                            lst.dismiss_confirm_dialog(cx);
                                        });
                                    }
                                })
                                .on_cancel({
                                    let l = list.clone();
                                    move |_window, cx| {
                                        l.update(cx, |lst, cx| lst.dismiss_confirm_dialog(cx));
                                    }
                                })
                                .into_any_element()
                        }
                        None => div().into_any_element(),
                    };

                    // Constrain to main panel bounds (left=36px offset for sidebar).
                    // ConfirmDialog fills this container and centers the modal card within it.
                    root.child(
                        div()
                            .absolute()
                            .left(px(36.))
                            .right(px(0.))
                            .top(px(0.))
                            .bottom(px(0.))
                            .child(dialog_element),
                    )
                },
            )
    }
}
