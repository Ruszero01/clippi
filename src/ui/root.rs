//! Root view — the main window's top-level component.
//!
//! --- Matches the original Slint `app.slint` layout: ---
//! --- - Transparent window background ---
//! --- - Sidebar at x=0, y=84px in the transparent margin ---
//! --- - Main panel offset 36px from left, 12px border-radius, 1px border ---
//! --- - Titlebar + stacked views (clipboard / settings / edit) ---

use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_transitions::WindowUseTransition;

use crate::core::i18n_keys::I18nKey;
use crate::state::app::AppState;
use crate::ui::window_manager::{WindowManager, WindowManagerEvent};

/// Toast enter / exit animation duration.
const TOAST_ANIM_DURATION: Duration = Duration::from_millis(220);

use super::clipboard_list::{ClipboardListEvent, ClipboardListView, ConfirmDialogState};
use super::components::confirm_dialog::ConfirmDialog;
use super::components::toast::Toast;
use super::context_menu::{ContextMenu, MenuItemContext};
use super::edit_panel::{EditPanel, EditPanelEvent};
use super::search_bar::SearchBar;
use super::settings::hotkey;
use super::settings::{SettingsEvent, SettingsPanel};
use super::sidebar::Sidebar;
use super::tag_filter::{render_edit_panel, TagFilterPanel};
use super::tag_picker::TagPickerPanel;
use super::theme::ClippiTheme;
use super::titlebar::{Titlebar, TitlebarEvent};
use super::type_filter_config::TypeFilterConfigPanel;

pub struct RootView {
    state: Entity<AppState>,
    window_manager: Entity<WindowManager>,
    titlebar: Entity<Titlebar>,
    list_view: Entity<ClipboardListView>,
    search_bar: Entity<SearchBar>,
    settings_panel: Entity<SettingsPanel>,
    edit_panel: Entity<EditPanel>,
    sidebar: Entity<Sidebar>,
    tag_filter_panel: Entity<TagFilterPanel>,
    type_filter_config_panel: Entity<TypeFilterConfigPanel>,
    current_view: String,
    pinned: bool,
    theme: ClippiTheme,
    /// Cached at creation time so the ThemeChanged handler (which only
    /// has access to `&mut App`) can resolve the "system" theme correctly.
    window_appearance: WindowAppearance,
    last_edit_tag_id: i64,
    /// Timer that auto-dismisses the toast notification after a duration.
    _toast_timer: Option<Task<()>>,
    /// Timer that clears the toast after the exit animation completes.
    _toast_cleanup: Option<Task<()>>,
    /// Animation generation counter — bumps on show/dismiss for fresh transitions.
    toast_generation: u64,
    /// True while the exit animation is playing (toast still rendered but fading out).
    toast_dismissing: bool,
    /// Action buttons for the current toast (e.g. "Download" / "Later").
    toast_actions: Option<Vec<crate::ui::components::toast::ToastAction>>,
    /// When the toast should auto-dismiss (None = use default, Some = custom).
    toast_timer_expiry: Option<std::time::Instant>,
    /// Set to true on WindowHidden, cleared after auto-focusing the search bar.
    needs_auto_focus: bool,
    _wm_subscription: Subscription,
    _subscriptions: Vec<Subscription>,
    _appearance_subscription: Option<Subscription>,
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
        // Wire up search_bar reference for Ctrl+F keyboard shortcut.
        list_view.update(cx, |list, _cx| {
            list.search_bar = Some(search_bar.clone());
        });
        let settings_panel = cx.new(|cx| {
            SettingsPanel::new(
                state.clone(),
                window_manager.clone(),
                theme.clone(),
                window,
                cx,
            )
        });
        let edit_panel = cx.new(|cx| EditPanel::new(state.clone(), theme.clone(), window, cx));
        let sidebar = cx.new(|_cx| Sidebar::new(state.clone(), list_view.clone(), &theme));
        let tag_filter_panel = cx.new(|cx| {
            TagFilterPanel::new(
                state.clone(),
                list_view.clone(),
                search_bar.clone(),
                window,
                cx,
            )
        });
        let type_filter_config_panel =
            cx.new(|cx| TypeFilterConfigPanel::new(state.clone(), search_bar.clone(), window, cx));

        // Subscribe to WindowManager events for clipboard changes and pin state.
        let _wm_subscription = cx.subscribe(
            &window_manager,
            move |this, _wm, event: &WindowManagerEvent, cx| match event {
                WindowManagerEvent::ClipboardChanged => {
                    let items = this.state.read(cx).items.clone();
                    let scroll_to_top = this.state.read(cx).settings.auto_scroll_to_top;
                    this.list_view.update(cx, |list, cx| {
                        list.refresh_settings_from_state(scroll_to_top, cx);
                        list.set_items(items, cx);
                    });
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
                WindowManagerEvent::OpenVersionSettings => {
                    this.current_view = "settings".into();
                    this.settings_panel
                        .update(cx, |panel, _cx| panel.set_active_tab(5));
                    this.search_bar
                        .update(cx, |bar, cx| bar.close_tag_panel(cx));
                    cx.notify();
                }
                WindowManagerEvent::HotkeyRecordingComplete => {
                    // --- Notify SettingsPanel so it re-renders with the updated ---
                    // --- hotkey display and recording state from AppState. ---
                    this.settings_panel.update(cx, |_panel, cx| {
                        cx.notify();
                    });
                }
                WindowManagerEvent::SyncChanged => {
                    this.settings_panel.update(cx, |_panel, cx| {
                        cx.notify();
                    });
                    cx.notify();
                }
                WindowManagerEvent::PasteShortcutRecorded { app_name, shortcut } => {
                    this.settings_panel.update(cx, |panel, cx| {
                        panel.recording_paste_shortcut = None;
                        cx.emit(SettingsEvent::ShowHotkeyConfirm(
                            hotkey::HotkeyConfirmAction::AddPasteShortcut {
                                app_name: app_name.clone(),
                                shortcut: shortcut.clone(),
                            },
                        ));
                    });
                    cx.notify();
                }
                WindowManagerEvent::WindowHidden => {
                    this.needs_auto_focus = true;
                    this.list_view.update(cx, |list, cx| {
                        list.dismiss_all_panels(cx);
                    });
                    this.search_bar.update(cx, |bar, cx| {
                        bar.close_tag_panel(cx);
                    });
                }
                #[cfg(target_os = "windows")]
                WindowManagerEvent::DpiChanged => {
                    cx.notify();
                }
                WindowManagerEvent::UpdateAvailable => {
                    let version = this
                        .state
                        .read(cx)
                        .update_available
                        .as_ref()
                        .map(|i| i.latest_version.clone())
                        .unwrap_or_default();
                    let msg = I18nKey::ToastUpdateAvailable
                        .text()
                        .replace("{0}", &version);
                    this.state.update(cx, |s, _cx| s.toast_message = Some(msg));
                    this.toast_actions = Some(vec![
                        crate::ui::components::toast::ToastAction {
                            label: I18nKey::BtnDownload.text().to_string(),
                            on_click: {
                                let wm = this.window_manager.clone();
                                Rc::new(move |_window, cx| {
                                    wm.update(cx, |wm, cx| wm.start_update_download(cx));
                                })
                            },
                            primary: true,
                        },
                        crate::ui::components::toast::ToastAction {
                            label: I18nKey::BtnLater.text().to_string(),
                            on_click: Rc::new(|_window, _cx| {}),
                            primary: false,
                        },
                    ]);
                    this.toast_timer_expiry =
                        Some(std::time::Instant::now() + Duration::from_secs(15));
                    cx.notify();
                }
                WindowManagerEvent::UpdateProgress(phase) => {
                    use crate::services::update::UpdatePhase;
                    match phase {
                        UpdatePhase::Downloading { progress } => {
                            let msg = I18nKey::VersionDownloading
                                .text()
                                .replace("{0}", &progress.to_string());
                            this.state.update(cx, |s, _cx| s.toast_message = Some(msg));
                            this.toast_actions = Some(Vec::new());
                            this.toast_timer_expiry =
                                Some(std::time::Instant::now() + Duration::from_secs(3600));
                        }
                        UpdatePhase::ReadyToRestart => {
                            this.state.update(cx, |s, _cx| {
                                s.toast_message = Some(I18nKey::VersionReady.text().to_string())
                            });
                            this.toast_actions =
                                Some(vec![crate::ui::components::toast::ToastAction {
                                    label: I18nKey::BtnRestartNow.text().to_string(),
                                    on_click: {
                                        let wm = this.window_manager.clone();
                                        Rc::new(move |_window, cx| {
                                            wm.update(cx, |wm, cx| wm.do_update_restart(cx));
                                        })
                                    },
                                    primary: true,
                                }]);
                            this.toast_timer_expiry =
                                Some(std::time::Instant::now() + Duration::from_secs(3600));
                        }
                        UpdatePhase::Error(msg) => {
                            let err_msg = I18nKey::ToastUpdateError.text().replace("{0}", msg);
                            this.state
                                .update(cx, |s, _cx| s.toast_message = Some(err_msg));
                            this.toast_actions = Some(Vec::new());
                            this.toast_timer_expiry =
                                Some(std::time::Instant::now() + Duration::from_secs(5));
                        }
                        UpdatePhase::UpToDate => {
                            // Silent — no toast for "up to date".
                        }
                        _ => {}
                    }
                    cx.notify();
                }
            },
        );

        let wm = window_manager.clone();
        let titlebar_for_events = titlebar.clone();
        let backend_panel = settings_panel.read(cx).backend_panel();
        let _subscriptions = vec![
            cx.observe(&search_bar, |_this, _, cx| {
                cx.notify();
            }),
            cx.observe(&tag_filter_panel, |_this, _, cx| {
                cx.notify();
            }),
            cx.observe(&type_filter_config_panel, |_this, _, cx| {
                cx.notify();
            }),
            cx.observe(&backend_panel, |_this, _, cx| {
                cx.notify();
            }),
            cx.subscribe(
                &list_view,
                move |this, _list, event: &ClipboardListEvent, cx| match event {
                    ClipboardListEvent::OpenEdit(id) => {
                        let opened = this
                            .state
                            .update(cx, |state, _cx| state.start_edit_item(*id));
                        if opened {
                            this.current_view = "edit".into();
                            this.search_bar
                                .update(cx, |bar, cx| bar.close_tag_panel(cx));
                            cx.notify();
                        }
                    }
                },
            ),
            cx.subscribe(
                &edit_panel,
                move |this, _panel, event: &EditPanelEvent, cx| {
                    match event {
                        EditPanelEvent::Back => {
                            this.state.update(cx, |state, _cx| state.cancel_edit_item());
                        }
                        EditPanelEvent::Saved => {
                            let items = this.state.read(cx).items.clone();
                            this.list_view.update(cx, |list, cx| {
                                list.set_items(items, cx);
                            });
                        }
                    }
                    this.current_view = "clipboard".into();
                    cx.notify();
                },
            ),
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
                        // --- Use cached window_appearance from creation time so ---
                        // --- "system" theme resolves correctly even though we ---
                        // --- only have &mut App here (not WindowContext). ---
                        this.theme =
                            ClippiTheme::from_setting(theme_str, Some(this.window_appearance));
                        let theme = this.theme.clone();

                        // --- Sync gpui_component theme so that Input, Scrollbar ---
                        // --- and other gpui_component widgets follow our theme. ---
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
                        // --- Must restore transparent background after Theme::change ---
                        // --- resets it — otherwise the window loses transparency. ---
                        gpui_component::Theme::global_mut(cx).background =
                            Hsla::transparent_black();

                        this.titlebar.update(cx, |titlebar, cx| {
                            titlebar.set_theme(theme.clone(), cx);
                        });
                        this.search_bar.update(cx, |search_bar, cx| {
                            search_bar.set_theme(theme.clone(), cx);
                        });
                        this.list_view.update(cx, |list_view, cx| {
                            list_view.set_theme(theme.clone(), cx);
                        });
                        this.settings_panel.update(cx, |panel, cx| {
                            panel.reload_theme(theme.clone(), cx);
                        });
                        this.edit_panel.update(cx, |panel, cx| {
                            panel.set_theme(theme.clone(), cx);
                        });
                        this.sidebar.update(cx, |sidebar, cx| {
                            sidebar.set_theme(&theme, cx);
                        });
                        cx.notify();
                    }
                    SettingsEvent::ClipboardSettingsChanged {
                        reload_items,
                        scroll_to_top,
                    } => {
                        if *reload_items {
                            this.state.update(cx, |state, _cx| state.reload_items());
                            let items = this.state.read(cx).items.clone();
                            this.list_view.update(cx, |list_view, cx| {
                                list_view.refresh_settings_from_state(*scroll_to_top, cx);
                                list_view.set_items(items, cx);
                            });
                        } else {
                            this.list_view.update(cx, |list_view, cx| {
                                list_view.refresh_settings_from_state(*scroll_to_top, cx);
                            });
                        }
                        cx.notify();
                    }
                    SettingsEvent::ShowHotkeyConfirm(action) => {
                        this.settings_panel.update(cx, |panel, _cx| {
                            panel.hotkey_confirm = Some(action.clone());
                        });
                        cx.notify();
                    }
                    SettingsEvent::HotkeyPasteShortcut { ref action } => {
                        match action {
                            hotkey::HotkeyConfirmAction::AddPasteShortcut {
                                app_name,
                                shortcut,
                            } => {
                                let mut list = this.state.read(cx).settings.paste_shortcuts.clone();
                                // Remove existing entry for same app (overwrite)
                                list.retain(|e| e.app_name != *app_name);
                                list.push(crate::core::settings::PasteShortcutEntry {
                                    app_name: app_name.clone(),
                                    shortcut: shortcut.clone(),
                                });
                                this.state.update(cx, |s, _cx| {
                                    s.settings.paste_shortcuts = list;
                                    s.settings.save();
                                });
                                this.settings_panel.update(cx, |panel, cx| {
                                    panel.clear_paste_shortcut_state(cx);
                                    panel.clear_hotkey_confirm(cx);
                                });
                            }
                            hotkey::HotkeyConfirmAction::RemovePasteShortcut { app_name } => {
                                let mut list = this.state.read(cx).settings.paste_shortcuts.clone();
                                list.retain(|e| e.app_name != *app_name);
                                this.state.update(cx, |s, _cx| {
                                    s.settings.paste_shortcuts = list;
                                    s.settings.save();
                                });
                                this.settings_panel.update(cx, |panel, cx| {
                                    panel.clear_paste_shortcut_state(cx);
                                    panel.clear_hotkey_confirm(cx);
                                });
                            }
                            _ => {}
                        }
                        cx.notify();
                    }
                    SettingsEvent::DataError(msg) => {
                        this.state.update(cx, |s, _cx| {
                            s.toast_message = Some(format!("{}: {msg}", I18nKey::ErrDataOp.text()));
                        });
                        cx.notify();
                    }
                },
            ),
        ];

        // --- Subscribe to OS appearance changes so the "system" theme ---
        // --- updates in real-time without requiring an app restart. ---
        let appearance_sub = cx.observe_window_appearance(window, move |this, win, cx| {
            // Always refresh the cached appearance so the ThemeChanged
            // handler reads the current value even if the user switches
            // from an explicit theme to "system" after an OS change.
            this.window_appearance = win.appearance();

            // Only rebuild theme when the user has selected "system".
            if this.state.read(cx).settings.theme != "system" {
                return;
            }

            this.theme = ClippiTheme::from_setting("system", Some(this.window_appearance));
            let theme = this.theme.clone();

            // Sync gpui_component theme so that Input, Scrollbar and other
            // gpui_component widgets follow our theme.
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
            gpui_component::Theme::global_mut(cx).background = Hsla::transparent_black();

            // Propagate to child views (mirrors ThemeChanged handler).
            this.titlebar
                .update(cx, |tb, cx| tb.set_theme(theme.clone(), cx));
            this.search_bar
                .update(cx, |sb, cx| sb.set_theme(theme.clone(), cx));
            this.list_view
                .update(cx, |lv, cx| lv.set_theme(theme.clone(), cx));
            this.settings_panel
                .update(cx, |sp, cx| sp.reload_theme(theme.clone(), cx));
            this.edit_panel
                .update(cx, |ep, cx| ep.set_theme(theme.clone(), cx));
            this.sidebar.update(cx, |sb, cx| sb.set_theme(&theme, cx));

            cx.notify();
        });

        Self {
            state,
            window_manager,
            titlebar,
            list_view,
            search_bar,
            settings_panel,
            edit_panel,
            sidebar,
            tag_filter_panel,
            type_filter_config_panel,
            current_view: "clipboard".into(),
            pinned: false,
            theme,
            window_appearance,
            last_edit_tag_id: -1,
            _toast_timer: None,
            _toast_cleanup: None,
            toast_generation: 0,
            toast_dismissing: false,
            toast_actions: None,
            toast_timer_expiry: None,
            needs_auto_focus: true,
            _wm_subscription,
            _subscriptions,
            _appearance_subscription: Some(appearance_sub),
        }
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = self.sidebar.clone();
        let titlebar = self.titlebar.clone();
        let list_view = self.list_view.clone();
        let search_bar = self.search_bar.clone();
        let settings_panel = self.settings_panel.clone();
        let backend_panel = self.settings_panel.read(cx).backend_panel();
        let backend_panel_visible = backend_panel.read(cx).is_visible();
        let edit_panel = self.edit_panel.clone();
        let tag_filter_panel = self.tag_filter_panel.clone();
        let tag_panel_open = self.search_bar.read(cx).tag_panel_open();
        let is_clipboard = self.current_view == "clipboard";
        let is_settings = self.current_view == "settings";
        let is_edit = self.current_view == "edit";
        let theme = &self.theme;
        let panel_border = if theme.bg == rgb(0x191a1b) {
            rgb(0x3a3b3c)
        } else {
            rgb(0xd0d2de)
        };
        // 1 physical-pixel border, DPI-aware — matches OS system border width.
        // px(1.0) is 1 logical px which scales with monitor DPI; dividing by the
        // scale factor keeps the rendered border at device-pixel thickness.
        let scale = window.scale_factor();
        let border_width = px(1.5 / scale.max(1.0));

        // Actual window dimensions for positioning overlays
        let viewport = window.viewport_size();
        let win_w = f32::from(viewport.width);
        let win_h = f32::from(viewport.height);

        // --- Auto-focus search bar when the window opens ---
        if self.needs_auto_focus && is_clipboard {
            self.needs_auto_focus = false;
            if self.state.read(cx).settings.auto_focus_search {
                self.search_bar.update(cx, |bar, cx| {
                    bar.focus(window, cx);
                });
            }
        }

        // --- Toast state machine ---
        // --- Enter: bump generation → new transition (0 → 1 opacity, slide up). ---
        // --- Display: hold ~2.8s. ---
        // Exit: same generation, update target to 0 / slide-down → smooth reverse.
        // --- Cleanup: after transition completes, clear the message. ---
        {
            let has_toast = self.state.read(cx).toast_message.is_some();
            if has_toast && !self.toast_dismissing && self._toast_timer.is_none() {
                // Determine duration:
                //   Some(expiry) → dismiss at that moment
                //   None          → never auto-dismiss (pinned toast)
                let expiry = self.toast_timer_expiry.unwrap_or(
                    std::time::Instant::now() + super::components::toast::TOAST_DURATION,
                );
                let dur = expiry
                    .checked_duration_since(std::time::Instant::now())
                    .map(|d| d.saturating_sub(TOAST_ANIM_DURATION))
                    .unwrap_or(Duration::from_secs(0));
                if dur > Duration::from_secs(0) {
                    // Bump generation so the enter animation replays for a new message.
                    self.toast_generation = self.toast_generation.wrapping_add(1);
                    self._toast_timer =
                        Some(cx.spawn(async move |weak_self: WeakEntity<RootView>, cx| {
                            Timer::after(dur).await;
                            if let Some(this) = weak_self.upgrade() {
                                let _ = this.update(cx, |root, root_cx| {
                                    // --- Start exit animation — same generation so the ---
                                    // --- transition smoothly reverses from its current value. ---
                                    root.toast_dismissing = true;
                                    root_cx.notify();
                                });
                                // --- Cleanup after the exit transition finishes, plus a ---
                                // --- small grace period so the visual zero point is stable. ---
                                Timer::after(TOAST_ANIM_DURATION + Duration::from_millis(60)).await;
                                if let Some(this) = weak_self.upgrade() {
                                    let _ = this.update(cx, |root, root_cx| {
                                        root.state.update(root_cx, |s, _cx| s.clear_toast());
                                        root.toast_dismissing = false;
                                        root.toast_actions = None;
                                        root.toast_timer_expiry = None;
                                    });
                                }
                            }
                        }));
                }
            } else if !has_toast {
                self._toast_timer = None;
                self._toast_cleanup = None;
                self.toast_dismissing = false;
            }
        }

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
                    .border(border_width)
                    .border_color(panel_border)
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .occlude()
                    .child(titlebar)
                    .when(is_clipboard, |panel| {
                        panel.child(search_bar.clone()).child(list_view.clone())
                    })
                    .when(is_settings, |panel| panel.child(settings_panel))
                    .when(is_edit, |panel| panel.child(edit_panel)),
            )
            // --- Tag filter panel — ConfirmDialog pattern: ---
            // --- full-screen backdrop that closes on click outside, ---
            // --- panel positioned top-right, occlude prevents click-through. ---
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
            // --- Type filter config panel — same backdrop pattern as tag filter ---
            .when(
                self.search_bar.read(cx).filter_config_open() && is_clipboard,
                |root| {
                    let search_for_backdrop = search_bar.clone();
                    let config_panel = self.type_filter_config_panel.clone();
                    root.child(
                        div()
                            .absolute()
                            .size_full()
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                cx.stop_propagation();
                                search_for_backdrop
                                    .update(cx, |bar, cx| bar.close_filter_config(cx));
                            })
                            .child(
                                div()
                                    .absolute()
                                    .right(px(8.))
                                    .top(px(106.))
                                    .occlude()
                                    .child(config_panel),
                            ),
                    )
                },
            )
            // --- Tag edit overlay — centered in main panel area (left:36px) ---
            .when(
                {
                    let app_state = self.state.read(cx);
                    let editing_id = app_state.editing_tag_id;
                    if editing_id >= 0 && editing_id != self.last_edit_tag_id {
                        self.last_edit_tag_id = editing_id;
                        let edit_name = app_state.editing_tag_name.clone();
                        let edit_input = self.tag_filter_panel.read(cx).edit_name_input().clone();
                        edit_input.update(cx, |input, cx| {
                            input.set_value(&edit_name, window, cx);
                        });
                    }
                    editing_id >= 0 && is_clipboard
                },
                |root| {
                    let app_state = self.state.read(cx);
                    let editing_tag_id = app_state.editing_tag_id;
                    let editing_tag_color = app_state.editing_tag_color.clone();
                    let edit_name_input = self.tag_filter_panel.read(cx).edit_name_input().clone();
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
                                        tf.update(cx, |panel, cx| {
                                            panel.cancel_edit_tag(cx);
                                            cx.notify();
                                        });
                                    }
                                },
                                {
                                    let tf = tag_filter.clone();
                                    move |_w, cx, color| {
                                        tf.update(cx, |panel, cx| {
                                            panel.set_edit_tag_color(&color, cx);
                                            cx.notify();
                                        });
                                    }
                                },
                                {
                                    let tf = tag_filter.clone();
                                    move |_w, cx, name, color| {
                                        tf.update(cx, |panel, cx| {
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

                    // --- Backdrop — click to dismiss ---
                    root.child(
                        div()
                            .absolute()
                            .size_full()
                            .on_mouse_down(MouseButton::Left, {
                                let l = list.clone();
                                move |_ev, _window, cx| {
                                    cx.stop_propagation();
                                    l.update(cx, |lst, cx| lst.dismiss_context_menu(cx));
                                }
                            }),
                    )
                    .child(div().absolute().occlude().child({
                        let l = list_for_action.clone();
                        if is_batch {
                            let count = l.read(cx).selected_count;
                            ContextMenu::for_batch(count)
                                .with_position(menu_x, menu_y, win_w, win_h)
                                .theme(self.theme.clone())
                                .on_action({
                                    let l = l.clone();
                                    move |action, window, cx| {
                                        l.update(cx, |lst, cx| {
                                            lst.handle_menu_action(action, window, cx);
                                        });
                                    }
                                })
                                .on_dismiss({
                                    let l = l.clone();
                                    move |_window, cx| {
                                        l.update(cx, |lst, cx| {
                                            lst.hide_context_menu(cx);
                                        });
                                    }
                                })
                                .into_any_element()
                        } else if let Some(ref clip_item) = item {
                            let ctx = MenuItemContext::from_item(clip_item);
                            ContextMenu::for_item(&ctx)
                                .with_position(menu_x, menu_y, win_w, win_h)
                                .theme(self.theme.clone())
                                .on_action({
                                    let l = l.clone();
                                    move |action, window, cx| {
                                        l.update(cx, |lst, cx| {
                                            lst.handle_menu_action(action, window, cx);
                                        });
                                    }
                                })
                                .on_dismiss({
                                    let l = l.clone();
                                    move |_window, cx| {
                                        l.update(cx, |lst, cx| {
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
                self.list_view.read(cx).tag_picker_visible() && is_clipboard,
                |root| {
                    let list = self.list_view.clone();
                    let list_for_panel = self.list_view.clone();
                    let (picker_x, picker_y) = list.read(cx).tag_picker_position();
                    let is_batch = list.read(cx).tag_picker_is_batch();
                    let rows = list.update(cx, |list, cx| list.tag_picker_rows(cx));
                    let create_input = list.read(cx).tag_create_input().clone();
                    let clamped_x = picker_x.clamp(4.0, (win_w - 304.0 - 4.0).max(4.0));
                    let clamped_y = picker_y.clamp(4.0, (win_h - 300.0 - 4.0).max(4.0));

                    root.child(
                        div()
                            .absolute()
                            .size_full()
                            .on_mouse_down(MouseButton::Left, {
                                let l = list.clone();
                                move |_ev, _window, cx| {
                                    cx.stop_propagation();
                                    l.update(cx, |lst, cx| lst.hide_tag_picker(cx));
                                }
                            }),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(clamped_x))
                            .top(px(clamped_y))
                            .occlude()
                            .child(
                                TagPickerPanel::new(rows, is_batch, create_input, self.theme.clone())
                                    .on_toggle({
                                        let l = list_for_panel.clone();
                                        move |tag_id, state, _window, cx| {
                                            l.update(cx, |lst, cx| {
                                                lst.toggle_picker_tag(tag_id, state, cx);
                                            });
                                        }
                                    })
                                    .on_clear({
                                        let l = list_for_panel.clone();
                                        move |_window, cx| {
                                            l.update(cx, |lst, cx| {
                                                lst.clear_picker_tags(cx);
                                            });
                                        }
                                    })
                                    .on_close({
                                        let l = list_for_panel.clone();
                                        move |_window, cx| {
                                            l.update(cx, |lst, cx| {
                                                lst.hide_tag_picker(cx);
                                            });
                                        }
                                    })
                                    .on_create({
                                        let l = list_for_panel.clone();
                                        move |name, _window, cx| {
                                            l.update(cx, |lst, cx| {
                                                lst.create_tag_from_picker(&name, cx);
                                            });
                                        }
                                    }),
                            ),
                    )
                },
            )
            .when(
                self.list_view.read(cx).confirm_dialog_state().is_some() && is_clipboard,
                |root| {
                    let list = self.list_view.clone();
                    let app_state = self.state.clone();

                    // --- Read dialog state and clone what we need before closures ---
                    let dialog = list.read(cx).confirm_dialog_state().cloned();
                    let dialog_element: AnyElement = match dialog {
                        Some(ConfirmDialogState::DeleteSingle { id }) => {
                            ConfirmDialog::delete_single()
                                .theme(self.theme.clone())
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
                                .theme(self.theme.clone())
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
            // --- Settings hotkey blacklist ConfirmDialog ---
            .when(
                is_settings && self.settings_panel.read(cx).hotkey_confirm.is_some(),
                |root| {
                    let dialog_focus = cx.focus_handle();
                    let settings = self.settings_panel.clone();
                    let wm = self.window_manager.clone();
                    let app_state = self.state.clone();

                    let action = settings.read(cx).hotkey_confirm.clone();
                    let dialog_element: AnyElement = match action {
                        Some(hotkey::HotkeyConfirmAction::AddBlacklist { app_name }) => {
                            ConfirmDialog::add_blacklist(&app_name)
                                .theme(self.theme.clone())
                                .on_confirm({
                                    let wm = wm.clone();
                                    let app_state = app_state.clone();
                                    let settings = settings.clone();
                                    let app_name = app_name.clone();
                                    move |_window, cx| {
                                        let app_name = app_name.clone();
                                        app_state.update(cx, |s, _cx| {
                                            if !s.settings.hotkey_blacklist.contains(&app_name) {
                                                s.settings.hotkey_blacklist.push(app_name.clone());
                                                s.settings.save();
                                            }
                                        });
                                        // --- Sync WindowManager's blacklist from settings ---
                                        let updated =
                                            app_state.read(cx).settings.hotkey_blacklist.clone();
                                        wm.update(cx, |wm, _cx| {
                                            wm.set_blacklist(updated);
                                        });
                                        settings.update(cx, |panel, cx| {
                                            panel.clear_hotkey_confirm(cx);
                                        });
                                    }
                                })
                                .on_cancel({
                                    let settings = settings.clone();
                                    move |_window, cx| {
                                        settings.update(cx, |panel, cx| {
                                            panel.clear_hotkey_confirm(cx);
                                        });
                                    }
                                })
                                .focus_handle(dialog_focus.clone())
                                .into_any_element()
                        }
                        Some(hotkey::HotkeyConfirmAction::RemoveBlacklist { app_name }) => {
                            ConfirmDialog::remove_blacklist(&app_name)
                                .theme(self.theme.clone())
                                .on_confirm({
                                    let wm = wm.clone();
                                    let app_state = app_state.clone();
                                    let settings = settings.clone();
                                    let app_name = app_name.clone();
                                    move |_window, cx| {
                                        app_state.update(cx, |s, _cx| {
                                            s.settings.hotkey_blacklist.retain(|a| a != &app_name);
                                            s.settings.save();
                                        });
                                        // --- Sync WindowManager's blacklist from settings ---
                                        let updated =
                                            app_state.read(cx).settings.hotkey_blacklist.clone();
                                        wm.update(cx, |wm, _cx| {
                                            wm.set_blacklist(updated);
                                        });
                                        settings.update(cx, |panel, cx| {
                                            panel.clear_hotkey_confirm(cx);
                                        });
                                    }
                                })
                                .on_cancel({
                                    let settings = settings.clone();
                                    move |_window, cx| {
                                        settings.update(cx, |panel, cx| {
                                            panel.clear_hotkey_confirm(cx);
                                        });
                                    }
                                })
                                .focus_handle(dialog_focus.clone())
                                .into_any_element()
                        }
                        Some(hotkey::HotkeyConfirmAction::AddPasteShortcut { app_name, shortcut }) => {
                            ConfirmDialog::add_paste_shortcut(&app_name, &shortcut)
                                .theme(self.theme.clone())
                                .on_confirm({
                                    let settings = settings.clone();
                                    let app_name = app_name.clone();
                                    let shortcut = shortcut.clone();
                                    move |_window, cx| {
                                        settings.update(cx, |_panel, cx| {
                                            cx.emit(SettingsEvent::HotkeyPasteShortcut {
                                                action: hotkey::HotkeyConfirmAction::AddPasteShortcut {
                                                    app_name: app_name.clone(),
                                                    shortcut: shortcut.clone(),
                                                },
                                            });
                                        });
                                    }
                                })
                                .on_cancel({
                                    let settings = settings.clone();
                                    move |_window, cx| {
                                        settings.update(cx, |panel, cx| {
                                            panel.clear_paste_shortcut_state(cx);
                                            panel.clear_hotkey_confirm(cx);
                                        });
                                    }
                                })
                                .focus_handle(dialog_focus.clone())
                                .into_any_element()
                        }
                        Some(hotkey::HotkeyConfirmAction::RemovePasteShortcut { app_name }) => {
                            ConfirmDialog::remove_paste_shortcut(&app_name)
                                .theme(self.theme.clone())
                                .on_confirm({
                                    let settings = settings.clone();
                                    let app_name = app_name.clone();
                                    move |_window, cx| {
                                        settings.update(cx, |_panel, cx| {
                                            cx.emit(SettingsEvent::HotkeyPasteShortcut {
                                                action: hotkey::HotkeyConfirmAction::RemovePasteShortcut {
                                                    app_name: app_name.clone(),
                                                },
                                            });
                                        });
                                    }
                                })
                                .on_cancel({
                                    let settings = settings.clone();
                                    move |_window, cx| {
                                        settings.update(cx, |panel, cx| {
                                            panel.clear_paste_shortcut_state(cx);
                                            panel.clear_hotkey_confirm(cx);
                                        });
                                    }
                                })
                                .focus_handle(dialog_focus.clone())
                                .into_any_element()
                        }
                        None => div().into_any_element(),
                    };

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
            .when(is_settings && backend_panel_visible, |root| {
                root.child(
                    div()
                        .absolute()
                        .left(px(36.))
                        .right(px(0.))
                        .top(px(0.))
                        .bottom(px(0.))
                        .child(backend_panel),
                )
            })
            .when(
                {
                    let toast_visible = self.state.read(cx).toast_message.is_some();
                    // --- Keep the toast rendered during the exit animation so it can fade out. ---
                    (toast_visible || self.toast_dismissing) && is_clipboard
                },
                |root| {
                    let state = self.state.clone();
                    let message = self
                        .state
                        .read(cx)
                        .toast_message
                        .clone()
                        .unwrap_or_default();
                    let theme = self.theme.clone();
                    let toast_visible = self.state.read(cx).toast_message.is_some();

                    // --- Animated opacity & slide ---
                    let toast_key = self.toast_generation;
                    let opacity_target: f32 = if toast_visible && !self.toast_dismissing {
                        1.0
                    } else {
                        0.0
                    };
                    let slide_target: f32 = if toast_visible && !self.toast_dismissing {
                        0.0
                    } else {
                        12.0
                    };

                    let opacity_transition = window
                        .use_keyed_transition(
                            ("toast-opacity", toast_key),
                            cx,
                            TOAST_ANIM_DURATION,
                            move |_, _| 1.0 - opacity_target,
                        )
                        .with_easing(RootView::toast_ease_out);
                    opacity_transition.update(cx, |value, _cx| {
                        *value = opacity_target;
                    });
                    let opacity = *opacity_transition.evaluate(window, cx);

                    let slide_transition = window
                        .use_keyed_transition(
                            ("toast-slide-y", toast_key),
                            cx,
                            TOAST_ANIM_DURATION,
                            move |_, _| 12.0_f32,
                        )
                        .with_easing(RootView::toast_ease_out);
                    slide_transition.update(cx, |value, _cx| {
                        *value = slide_target;
                    });
                    let slide_y = *slide_transition.evaluate(window, cx);

                    let dismiss_state = self.toast_dismissing;
                    let actions = self.toast_actions.clone().unwrap_or_default();
                    let has_actions = !actions.is_empty();
                    root.child(
                        div()
                            .absolute()
                            .left(px(36.))
                            .right(px(0.))
                            .top(px(0.))
                            .bottom(px(0.))
                            .child(
                                div()
                                    .absolute()
                                    .left(px(20.))
                                    .right(px(20.))
                                    .bottom(px(50.0 + slide_y))
                                    .opacity(opacity)
                                    // Plain toasts: click-to-dismiss. Interactive toasts
                                    // (with action buttons): occlude to block click-through.
                                    .when(!has_actions, |el| {
                                        el.cursor(CursorStyle::PointingHand)
                                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                                state.update(cx, |s, _cx| s.clear_toast());
                                            })
                                    })
                                    .when(has_actions, |el| el.occlude())
                                    .when(dismiss_state, |el| el.cursor(CursorStyle::Arrow))
                                    .child(Toast::new(message).theme(theme).actions(actions)),
                            ),
                    )
                },
            )
    }
}

impl RootView {
    fn toast_ease_out(_delta: f32) -> f32 {
        1.0 - (1.0 - _delta).powi(3)
    }
}
