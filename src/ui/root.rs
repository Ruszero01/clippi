//! Root view — the main window's top-level component.
//!
//! --- Matches the original Slint `app.slint` layout: ---
//! --- - Transparent window background ---
//! --- - Sidebar at x=0, y=84px in the transparent margin ---
//! --- - Main panel offset 36px from left, 12px border-radius, 1px border ---
//! --- - Titlebar + stacked views (clipboard / settings / edit) ---

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_transitions::WindowUseTransition;

use crate::core::i18n_keys::I18nKey;
use crate::state::app::AppState;
use crate::ui::window_manager::{WindowManager, WindowManagerEvent};

/// Toast enter / exit animation duration.
const TOAST_ANIM_DURATION: Duration = Duration::from_millis(220);
/// Main content view transition duration.
const VIEW_ANIM_DURATION: Duration = Duration::from_millis(180);
/// Floating panel / dialog enter animation duration.
const OVERLAY_ANIM_DURATION: Duration = Duration::from_millis(150);

use super::bottom_bar::BottomBar;
use super::clipboard_list::{ClipboardListEvent, ClipboardListView, ConfirmDialogState};
use super::components::app_list_dialog::{
    render_app_list_dialog, AppListDialogEntry, AppListDialogParams,
};
use super::components::confirm_dialog::ConfirmDialog;
use super::components::spinner::activity_spinner;
use super::components::toast::Toast;
use super::context_menu::{ContextMenu, MenuItemContext};
use super::edit_panel::{EditPanel, EditPanelEvent};
use super::filter_bar::FilterBar;
use super::search_box::SearchBox;
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
    search_box: Entity<SearchBox>,
    filter_bar: Entity<FilterBar>,
    settings_panel: Entity<SettingsPanel>,
    edit_panel: Entity<EditPanel>,
    sidebar: Entity<Sidebar>,
    tag_filter_panel: Entity<TagFilterPanel>,
    type_filter_config_panel: Entity<TypeFilterConfigPanel>,
    current_view: String,
    /// Page to restore when the transfer station is closed from the list view.
    /// Only set when the station was opened from the settings page.
    transfer_return_view: Option<String>,
    view_transition_generation: u64,
    view_transition_started: Option<Instant>,
    overlay_transitions: HashMap<&'static str, OverlayTransitionState>,
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
    /// Last toast message — used to detect when a new toast replaces the current
    /// one, cancelling the old timer and restarting a fresh one.
    last_toast_message: Option<String>,
    /// Set to true on WindowHidden, cleared after auto-focusing the search bar.
    needs_auto_focus: bool,
    /// Root-level clear-data dialog state, outside the clipped settings list.
    clear_data_confirm: bool,
    clear_data_include_favorites: bool,
    _wm_subscription: Subscription,
    _subscriptions: Vec<Subscription>,
    _appearance_subscription: Option<Subscription>,
    /// Focus handle for global keyboard events (ESC navigation).
    focus_handle: FocusHandle,
}

#[derive(Clone, Copy, Default)]
struct OverlayTransitionState {
    visible: bool,
    generation: u64,
    started_at: Option<Instant>,
}

impl RootView {
    pub fn new(
        window: &mut Window,
        state: Entity<AppState>,
        window_manager: Entity<WindowManager>,
        cx: &mut Context<Self>,
    ) -> Self {
        let app_state = state.read(cx);
        let items = app_state.visible_items();
        let window_appearance = window.appearance();
        let theme = ClippiTheme::from_setting(&app_state.settings.theme, Some(window_appearance));
        let list_view = cx.new(|cx| {
            ClipboardListView::new(
                items,
                state.clone(),
                theme.clone(),
                window,
                cx,
                window_manager.clone(),
            )
        });
        list_view.update(cx, |list, _cx| list.focus(window));
        let titlebar = cx.new(|_cx| Titlebar::new(state.clone(), list_view.clone(), theme.clone()));
        let search_box = cx
            .new(|cx| SearchBox::new(state.clone(), list_view.clone(), theme.clone(), window, cx));
        let filter_bar =
            cx.new(|_cx| FilterBar::new(state.clone(), list_view.clone(), theme.clone()));
        // Wire up search_box reference for Ctrl+F keyboard shortcut.
        list_view.update(cx, |list, _cx| {
            list.search_bar = Some(search_box.clone());
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
                filter_bar.clone(),
                window,
                cx,
            )
        });
        let type_filter_config_panel =
            cx.new(|cx| TypeFilterConfigPanel::new(state.clone(), filter_bar.clone(), window, cx));

        // Subscribe to WindowManager events for clipboard changes and pin state.
        let _wm_subscription = cx.subscribe(
            &window_manager,
            move |this, _wm, event: &WindowManagerEvent, cx| match event {
                WindowManagerEvent::ClipboardChanged => {
                    let items = this.state.read(cx).visible_items();
                    let scroll_to_top = this.state.read(cx).settings.auto_scroll_to_top;
                    this.list_view.update(cx, |list, cx| {
                        list.refresh_settings_from_state(scroll_to_top, cx);
                        list.set_items(items, cx);
                    });
                    cx.notify();
                }
                WindowManagerEvent::TransferStateChanged => {
                    // Sync list items in-place (like tag ops) — preserves scroll position.
                    this.list_view.update(cx, |list, cx| {
                        list.sync_items_from_state(cx);
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
                    this.switch_view("settings");
                    this.filter_bar
                        .update(cx, |bar, cx| bar.close_tag_panel(cx));
                    cx.notify();
                }
                WindowManagerEvent::OpenVersionSettings => {
                    // Clear any existing update toast — the version info is now
                    // visible on screen and a redundant toast is distracting.
                    if this.toast_actions.is_some() {
                        this._toast_timer = None;
                        this.toast_timer_expiry = None;
                        this.toast_actions = None;
                        this.state.update(cx, |s, _cx| s.clear_toast());
                    }
                    this.switch_view("settings");
                    this.settings_panel
                        .update(cx, |panel, _cx| panel.set_active_tab(5));
                    this.filter_bar
                        .update(cx, |bar, cx| bar.close_tag_panel(cx));
                    cx.notify();
                }
                WindowManagerEvent::HotkeyRecordingComplete => {
                    // --- Notify SettingsPanel so it re-renders with the updated ---
                    // --- hotkey display and recording state from AppState. ---
                    this.list_view.update(cx, |list, cx| {
                        list.finish_hotkey_recording_ui(cx);
                        list.sync_items_from_state(cx);
                    });
                    this.settings_panel.update(cx, |panel, cx| {
                        panel.recording_paste_shortcut = None;
                        cx.notify();
                    });
                    cx.notify();
                }
                WindowManagerEvent::SyncChanged => {
                    this.settings_panel.update(cx, |_panel, cx| {
                        cx.notify();
                    });
                    this.titlebar.update(cx, |_titlebar, cx| {
                        cx.notify();
                    });
                    cx.notify();
                }
                WindowManagerEvent::FileStatusChanged => {
                    let path_type_filter_active = {
                        let state = this.state.read(cx);
                        state.filters.is_type_active("file") || state.filters.is_type_active("path")
                    };
                    if path_type_filter_active {
                        this.state.update(cx, |state, _cx| state.reload_items());
                        this.list_view
                            .update(cx, |list, cx| list.sync_items_from_state(cx));
                    } else {
                        this.list_view.update(cx, |_list, cx| cx.notify());
                    }
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
                WindowManagerEvent::ResetToClipboard => {
                    this.switch_view("clipboard");
                    cx.notify();
                }
                WindowManagerEvent::WindowHidden => {
                    this.needs_auto_focus = true;
                    this.list_view.update(cx, |list, cx| {
                        list.release_items_for_hide(cx);
                    });
                    this.filter_bar.update(cx, |bar, cx| {
                        bar.close_tag_panel(cx);
                    });
                    _wm.update(cx, |wm, _cx| {
                        wm.cancel_paste_shortcut_recording();
                    });
                    this.settings_panel.update(cx, |panel, cx| {
                        panel.clear_paste_shortcut_state(cx);
                        panel.close_app_list_popups();
                        panel.latest_hotkeys_popup_open = false;
                        panel.hotkey_confirm = None;
                        cx.notify();
                    });
                }
                #[cfg(target_os = "windows")]
                WindowManagerEvent::DpiChanged => {
                    cx.notify();
                }
                WindowManagerEvent::UpdateAvailable => {
                    // Don't show the toast when the user is already looking at
                    // the version tab — the update info is right there on screen.
                    let on_version_tab = this.current_view == "settings"
                        && this.settings_panel.read(cx).active_tab() == 5;
                    if on_version_tab {
                        // AppState.update_available was already set by poll_update;
                        // notify so the version tab re-renders with the latest info.
                        cx.notify();
                        return;
                    }
                    // A newly discovered update must not inherit the timer from
                    // an unrelated toast that happened to be visible already.
                    this._toast_timer = None;
                    this.toast_dismissing = false;
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
                            on_click: {
                                let state = this.state.clone();
                                Rc::new(move |_window, cx| {
                                    state.update(cx, |s, _cx| s.clear_toast());
                                })
                            },
                            primary: false,
                        },
                    ]);
                    this.toast_timer_expiry =
                        Some(std::time::Instant::now() + Duration::from_secs(15));
                    cx.notify();
                }
                WindowManagerEvent::UpdateProgress(phase) => {
                    // Don't show the toast when the user is already looking at
                    // the version tab — the update progress is right there on screen.
                    let on_version_tab = this.current_view == "settings"
                        && this.settings_panel.read(cx).active_tab() == 5;
                    if on_version_tab {
                        // AppState.update_phase was already set by poll_update
                        // or start_update_check; notify so the version tab
                        // re-renders with the latest progress.
                        cx.notify();
                        return;
                    }
                    use crate::services::update::UpdatePhase;
                    match phase {
                        UpdatePhase::Downloading { progress } => {
                            // The first progress event replaces the update prompt;
                            // subsequent percentage events keep the same timer.
                            if this
                                .toast_actions
                                .as_ref()
                                .is_some_and(|actions| !actions.is_empty())
                            {
                                this._toast_timer = None;
                                this.toast_dismissing = false;
                            }
                            let msg = I18nKey::VersionDownloading
                                .text()
                                .replace("{0}", &progress.to_string());
                            this.state.update(cx, |s, _cx| s.toast_message = Some(msg));
                            this.toast_actions = Some(Vec::new());
                            this.toast_timer_expiry =
                                Some(std::time::Instant::now() + Duration::from_secs(3600));
                        }
                        UpdatePhase::Verifying => {
                            this.state.update(cx, |s, _cx| {
                                s.toast_message = Some(I18nKey::VersionVerifying.text().to_string())
                            });
                            this.toast_actions = Some(Vec::new());
                        }
                        UpdatePhase::Installing => {
                            this.state.update(cx, |s, _cx| {
                                s.toast_message =
                                    Some(I18nKey::VersionInstalling.text().to_string())
                            });
                            this.toast_actions = Some(Vec::new());
                        }
                        UpdatePhase::ReadyToRestart => {
                            this._toast_timer = None;
                            this.toast_dismissing = false;
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
                            this._toast_timer = None;
                            this.toast_dismissing = false;
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
                WindowManagerEvent::BitmapPasteFinished => {
                    let preparing = I18nKey::ToastPreparingBitmapImage.text();
                    let current = this.state.read(cx).toast_message.clone();
                    if current.as_deref() == Some(preparing) {
                        this._toast_timer = None;
                        this.toast_timer_expiry = None;
                        this.toast_actions = None;
                        this.state.update(cx, |s, _cx| s.clear_toast());
                        cx.notify();
                    }
                }
                WindowManagerEvent::DataMaintenanceToast(message) => {
                    this._toast_timer = None;
                    this.toast_dismissing = false;
                    this.state.update(cx, |state, _cx| {
                        state.toast_message = Some(message.clone());
                    });
                    this.toast_actions = None;
                    this.toast_timer_expiry =
                        Some(std::time::Instant::now() + Duration::from_secs(5));
                    cx.notify();
                }
            },
        );

        let wm = window_manager.clone();
        let titlebar_for_events = titlebar.clone();
        let backend_panel = settings_panel.read(cx).backend_panel();
        let _subscriptions = vec![
            cx.observe(&search_box, |_this, _, cx| {
                cx.notify();
            }),
            cx.observe(&filter_bar, |_this, _, cx| {
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
                            this.switch_view("edit");
                            this.filter_bar
                                .update(cx, |bar, cx| bar.close_tag_panel(cx));
                            cx.notify();
                        }
                    }
                    ClipboardListEvent::RequestHide => {
                        this.window_manager.update(cx, |wm, cx| {
                            wm.hide(cx);
                        });
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
                            let items = this.state.read(cx).visible_items();
                            this.list_view.update(cx, |list, cx| {
                                list.set_items(items, cx);
                            });
                        }
                    }
                    this.switch_view("clipboard");
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
                        this.switch_view("settings");
                        this.filter_bar
                            .update(cx, |bar, cx| bar.close_tag_panel(cx));
                        cx.notify();
                    }
                    TitlebarEvent::ToggleTransfer => {
                        this.handle_toggle_transfer(cx);
                    }
                },
            ),
            cx.subscribe(
                &settings_panel,
                move |this, _panel, event: &SettingsEvent, cx| match event {
                    SettingsEvent::Back => {
                        this.switch_view("clipboard");
                        this.settings_panel.update(cx, |panel, cx| {
                            panel.close_app_list_popups();
                            panel.latest_hotkeys_popup_open = false;
                            panel.hotkey_confirm = None;
                            cx.notify();
                        });
                        cx.notify();
                    }
                    SettingsEvent::TabChanged(idx) => {
                        // When the user clicks the version tab manually, clear
                        // any existing update toast — the info is now on screen.
                        if *idx == 5 && this.toast_actions.is_some() {
                            this._toast_timer = None;
                            this.toast_timer_expiry = None;
                            this.toast_actions = None;
                            this.state.update(cx, |s, _cx| s.clear_toast());
                            cx.notify();
                        }
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
                        this.search_box.update(cx, |search_box, cx| {
                            search_box.set_theme(theme.clone(), cx);
                        });
                        this.filter_bar.update(cx, |filter_bar, cx| {
                            filter_bar.set_theme(theme.clone(), cx);
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
                            let items = this.state.read(cx).visible_items();
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
                        log::error!("data settings error: {msg}");
                        this.state.update(cx, |s, _cx| {
                            s.toast_message = Some(I18nKey::ErrDataOp.text().to_string());
                        });
                        cx.notify();
                    }
                    SettingsEvent::DataToast(msg) => {
                        this.state.update(cx, |s, _cx| {
                            s.toast_message = Some(msg.clone());
                        });
                        cx.notify();
                    }
                    SettingsEvent::ShowClearDataConfirm => {
                        this.clear_data_confirm = true;
                        this.clear_data_include_favorites = false;
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
            this.search_box
                .update(cx, |sb, cx| sb.set_theme(theme.clone(), cx));
            this.filter_bar
                .update(cx, |fb, cx| fb.set_theme(theme.clone(), cx));
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
            search_box,
            filter_bar,
            settings_panel,
            edit_panel,
            sidebar,
            tag_filter_panel,
            type_filter_config_panel,
            current_view: "clipboard".into(),
            transfer_return_view: None,
            view_transition_generation: 0,
            view_transition_started: None,
            overlay_transitions: HashMap::new(),
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
            last_toast_message: None,
            needs_auto_focus: true,
            clear_data_confirm: false,
            clear_data_include_favorites: false,
            _wm_subscription,
            _subscriptions,
            _appearance_subscription: Some(appearance_sub),
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root_entity = cx.entity().clone();
        let sidebar = self.sidebar.clone();
        let titlebar = self.titlebar.clone();
        let list_view = self.list_view.clone();
        let search_box = self.search_box.clone();
        let filter_bar = self.filter_bar.clone();
        let settings_panel = self.settings_panel.clone();
        let backend_panel = self.settings_panel.read(cx).backend_panel();
        let backend_panel_open = backend_panel.read(cx).is_visible();
        let edit_panel = self.edit_panel.clone();
        let tag_filter_panel = self.tag_filter_panel.clone();
        let tag_panel_open = self.filter_bar.read(cx).tag_panel_open();
        let is_clipboard = self.current_view == "clipboard";
        let is_settings = self.current_view == "settings";
        let is_edit = self.current_view == "edit";
        let theme = self.theme.clone();
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

        let filter_config_open = self.filter_bar.read(cx).filter_config_open();
        let context_menu_open = self.list_view.read(cx).context_menu_visible();
        let tag_picker_open = self.list_view.read(cx).tag_picker_visible();
        let confirm_dialog_open = self.list_view.read(cx).confirm_dialog_state().is_some();
        let hotkey_confirm_open = self.settings_panel.read(cx).hotkey_confirm.is_some();
        let (
            latest_hotkeys_popup_open,
            hotkey_blacklist_popup_open,
            paste_shortcuts_popup_open,
            app_blacklist_popup_open,
            active_settings_tab,
        ) = {
            let panel = self.settings_panel.read(cx);
            (
                panel.latest_hotkeys_popup_open,
                panel.hotkey_blacklist_popup_open,
                panel.paste_shortcuts_popup_open,
                panel.app_blacklist_popup_open,
                panel.active_tab(),
            )
        };

        let editing_tag_visible = {
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
        };

        let tag_panel_visible = tag_panel_open && is_clipboard;
        let filter_config_visible = filter_config_open && is_clipboard;
        let context_menu_visible = context_menu_open && is_clipboard;
        let tag_picker_visible = tag_picker_open && is_clipboard;
        let confirm_dialog_visible = confirm_dialog_open && is_clipboard;
        let hotkey_confirm_visible = !is_edit && hotkey_confirm_open;
        let clear_data_confirm_visible = is_settings && self.clear_data_confirm;
        let backend_panel_visible = is_settings && backend_panel_open;
        let hotkey_blacklist_popup_visible =
            is_settings && active_settings_tab == 2 && hotkey_blacklist_popup_open;
        let paste_shortcuts_popup_visible =
            is_settings && active_settings_tab == 2 && paste_shortcuts_popup_open;
        let app_blacklist_popup_visible =
            is_settings && active_settings_tab == 1 && app_blacklist_popup_open;
        let latest_hotkeys_popup_visible = is_settings && latest_hotkeys_popup_open;
        let transfer_refreshing = is_clipboard && {
            let state = self.state.read(cx);
            state.transfer_filter_active && state.transfer_refreshing
        };

        let view_animating =
            Self::animation_running(self.view_transition_started, VIEW_ANIM_DURATION);
        let view_key = self.view_transition_generation;
        let view_opacity = if view_animating {
            Self::transition_f32(
                window,
                cx,
                ("root-view-opacity", view_key),
                VIEW_ANIM_DURATION,
                0.0,
                1.0,
            )
        } else {
            1.0
        };
        let view_offset = if view_animating {
            Self::transition_f32(
                window,
                cx,
                ("root-view-offset", view_key),
                VIEW_ANIM_DURATION,
                6.0,
                0.0,
            )
        } else {
            0.0
        };

        let tag_panel_gen = self.overlay_generation("tag-filter", tag_panel_visible);
        let filter_config_gen =
            self.overlay_generation("type-filter-config", filter_config_visible);
        let edit_tag_gen = self.overlay_generation("tag-edit", editing_tag_visible);
        let context_menu_gen = self.overlay_generation("context-menu", context_menu_visible);
        let tag_picker_gen = self.overlay_generation("tag-picker", tag_picker_visible);
        let confirm_dialog_gen = self.overlay_generation("confirm-dialog", confirm_dialog_visible);
        let hotkey_confirm_gen = self.overlay_generation("hotkey-confirm", hotkey_confirm_visible);
        let clear_data_confirm_gen =
            self.overlay_generation("clear-data-confirm", clear_data_confirm_visible);
        let backend_panel_gen = self.overlay_generation("backend-panel", backend_panel_visible);
        let hotkey_blacklist_popup_gen =
            self.overlay_generation("hotkey-blacklist-popup", hotkey_blacklist_popup_visible);
        let paste_shortcuts_popup_gen =
            self.overlay_generation("paste-shortcuts-popup", paste_shortcuts_popup_visible);
        let app_blacklist_popup_gen =
            self.overlay_generation("app-blacklist-popup", app_blacklist_popup_visible);
        let latest_hotkeys_popup_gen =
            self.overlay_generation("latest-hotkeys-popup", latest_hotkeys_popup_visible);

        let tag_panel_animating = self.overlay_animating("tag-filter");
        let filter_config_animating = self.overlay_animating("type-filter-config");
        let edit_tag_animating = self.overlay_animating("tag-edit");
        let context_menu_animating = self.overlay_animating("context-menu");
        let tag_picker_animating = self.overlay_animating("tag-picker");
        let _confirm_dialog_animating = self.overlay_animating("confirm-dialog");
        let hotkey_blacklist_popup_animating = self.overlay_animating("hotkey-blacklist-popup");
        let paste_shortcuts_popup_animating = self.overlay_animating("paste-shortcuts-popup");
        let app_blacklist_popup_animating = self.overlay_animating("app-blacklist-popup");
        let _hotkey_confirm_animating = self.overlay_animating("hotkey-confirm");
        let backend_panel_animating = self.overlay_animating("backend-panel");
        let latest_hotkeys_popup_animating = self.overlay_animating("latest-hotkeys-popup");

        let tag_panel_opacity = if tag_panel_visible && tag_panel_animating {
            Self::overlay_opacity(window, cx, tag_panel_gen, "tag-filter")
        } else {
            1.0
        };
        let tag_panel_offset = if tag_panel_visible && tag_panel_animating {
            Self::overlay_offset(window, cx, tag_panel_gen, "tag-filter")
        } else {
            0.0
        };
        let filter_config_opacity = if filter_config_visible && filter_config_animating {
            Self::overlay_opacity(window, cx, filter_config_gen, "type-filter-config")
        } else {
            1.0
        };
        let filter_config_offset = if filter_config_visible && filter_config_animating {
            Self::overlay_offset(window, cx, filter_config_gen, "type-filter-config")
        } else {
            0.0
        };
        let edit_tag_opacity = if editing_tag_visible && edit_tag_animating {
            Self::overlay_opacity(window, cx, edit_tag_gen, "tag-edit")
        } else {
            1.0
        };
        let edit_tag_scale = if editing_tag_visible && edit_tag_animating {
            Self::overlay_scale(window, cx, edit_tag_gen, "tag-edit")
        } else {
            1.0
        };
        let context_menu_opacity = if context_menu_visible && context_menu_animating {
            Self::overlay_opacity(window, cx, context_menu_gen, "context-menu")
        } else {
            1.0
        };
        let context_menu_offset = if context_menu_visible && context_menu_animating {
            Self::overlay_offset(window, cx, context_menu_gen, "context-menu")
        } else {
            0.0
        };
        let tag_picker_opacity = if tag_picker_visible && tag_picker_animating {
            Self::overlay_opacity(window, cx, tag_picker_gen, "tag-picker")
        } else {
            1.0
        };
        let tag_picker_offset = if tag_picker_visible && tag_picker_animating {
            Self::overlay_offset(window, cx, tag_picker_gen, "tag-picker")
        } else {
            0.0
        };
        let backend_panel_opacity = if backend_panel_visible && backend_panel_animating {
            Self::overlay_opacity(window, cx, backend_panel_gen, "backend-panel")
        } else {
            1.0
        };
        let latest_hotkeys_popup_opacity =
            if latest_hotkeys_popup_visible && latest_hotkeys_popup_animating {
                Self::overlay_opacity(window, cx, latest_hotkeys_popup_gen, "latest-hotkeys-popup")
            } else {
                1.0
            };
        let latest_hotkeys_popup_offset =
            if latest_hotkeys_popup_visible && latest_hotkeys_popup_animating {
                Self::overlay_offset(window, cx, latest_hotkeys_popup_gen, "latest-hotkeys-popup")
            } else {
                0.0
            };
        let latest_hotkeys_popup_scale =
            if latest_hotkeys_popup_visible && latest_hotkeys_popup_animating {
                Self::overlay_scale(window, cx, latest_hotkeys_popup_gen, "latest-hotkeys-popup")
            } else {
                1.0
            };
        let hotkey_blacklist_popup_opacity =
            if hotkey_blacklist_popup_visible && hotkey_blacklist_popup_animating {
                Self::overlay_opacity(
                    window,
                    cx,
                    hotkey_blacklist_popup_gen,
                    "hotkey-blacklist-popup",
                )
            } else {
                1.0
            };
        let hotkey_blacklist_popup_scale =
            if hotkey_blacklist_popup_visible && hotkey_blacklist_popup_animating {
                Self::overlay_scale(
                    window,
                    cx,
                    hotkey_blacklist_popup_gen,
                    "hotkey-blacklist-popup",
                )
            } else {
                1.0
            };
        let hotkey_blacklist_popup_offset =
            if hotkey_blacklist_popup_visible && hotkey_blacklist_popup_animating {
                Self::overlay_offset(
                    window,
                    cx,
                    hotkey_blacklist_popup_gen,
                    "hotkey-blacklist-popup",
                )
            } else {
                0.0
            };
        let paste_shortcuts_popup_opacity =
            if paste_shortcuts_popup_visible && paste_shortcuts_popup_animating {
                Self::overlay_opacity(
                    window,
                    cx,
                    paste_shortcuts_popup_gen,
                    "paste-shortcuts-popup",
                )
            } else {
                1.0
            };
        let paste_shortcuts_popup_scale =
            if paste_shortcuts_popup_visible && paste_shortcuts_popup_animating {
                Self::overlay_scale(
                    window,
                    cx,
                    paste_shortcuts_popup_gen,
                    "paste-shortcuts-popup",
                )
            } else {
                1.0
            };
        let paste_shortcuts_popup_offset =
            if paste_shortcuts_popup_visible && paste_shortcuts_popup_animating {
                Self::overlay_offset(
                    window,
                    cx,
                    paste_shortcuts_popup_gen,
                    "paste-shortcuts-popup",
                )
            } else {
                0.0
            };
        let app_blacklist_popup_opacity =
            if app_blacklist_popup_visible && app_blacklist_popup_animating {
                Self::overlay_opacity(window, cx, app_blacklist_popup_gen, "app-blacklist-popup")
            } else {
                1.0
            };
        let app_blacklist_popup_scale =
            if app_blacklist_popup_visible && app_blacklist_popup_animating {
                Self::overlay_scale(window, cx, app_blacklist_popup_gen, "app-blacklist-popup")
            } else {
                1.0
            };
        let app_blacklist_popup_offset =
            if app_blacklist_popup_visible && app_blacklist_popup_animating {
                Self::overlay_offset(window, cx, app_blacklist_popup_gen, "app-blacklist-popup")
            } else {
                0.0
            };

        // --- Auto-focus and clear search box when the window opens ---
        if self.needs_auto_focus && is_clipboard {
            self.needs_auto_focus = false;
            let clear_search = self.state.read(cx).settings.clear_search_on_show;
            let auto_focus = self.state.read(cx).settings.auto_focus_search;
            if clear_search {
                self.search_box.update(cx, |bar, cx| {
                    bar.clear_text(window, cx);
                });
            }
            if auto_focus {
                self.search_box.update(cx, |bar, cx| {
                    bar.focus(window, cx);
                });
            } else {
                self.list_view.update(cx, |list, _cx| list.focus(window));
            }
        }

        // --- Toast state machine ---
        // --- Enter: bump generation → new transition (0 → 1 opacity, slide up). ---
        // --- Display: hold ~2.8s. ---
        // Exit: same generation, update target to 0 / slide-down → smooth reverse.
        // --- Cleanup: after transition completes, clear the message. ---
        {
            let current = self.state.read(cx).toast_message.clone();
            let has_toast = current.is_some();
            let is_new_toast = current != self.last_toast_message;
            if is_new_toast {
                // Cancel any running timer from the previous toast.
                self._toast_timer = None;
                self.toast_dismissing = false;
            }
            self.last_toast_message = current.clone();

            if has_toast && !self.toast_dismissing && self._toast_timer.is_none() {
                if current.as_deref() == Some(I18nKey::ToastPreparingBitmapImage.text()) {
                    if is_new_toast {
                        self.toast_generation = self.toast_generation.wrapping_add(1);
                    }
                    self.toast_timer_expiry = None;
                    self.toast_actions = None;
                } else {
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
                                    Timer::after(TOAST_ANIM_DURATION + Duration::from_millis(60))
                                        .await;
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
                }
            } else if !has_toast {
                self._toast_timer = None;
                self._toast_cleanup = None;
                self.toast_dismissing = false;
            }
        }

        let root_focus = self.focus_handle.clone();
        let root_this = cx.entity().clone();
        let root_list = list_view.clone();
        let root_state = self.state.clone();
        let root_backend = backend_panel.clone();

        div()
            .relative()
            .size_full()
            .track_focus(&root_focus)
            .on_key_down(move |ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key.as_str() != "escape" {
                    return;
                }
                let cancelled_recording = root_this.update(cx, |this, cx| {
                    this.window_manager
                        .update(cx, |wm, cx| wm.cancel_active_hotkey_recording(cx))
                });
                if cancelled_recording {
                    cx.stop_propagation();
                    return;
                }
                let is_settings = root_this.read(cx).current_view == "settings";
                let is_edit = root_this.read(cx).current_view == "edit";
                if is_settings {
                    // Close backend panel popup first, then go back to clipboard
                    if root_backend.read(cx).is_visible() {
                        root_backend.update(cx, |panel, cx| panel.close(cx));
                    } else {
                        root_this.update(cx, |this, cx| {
                            this.switch_view("clipboard");
                            cx.notify();
                        });
                        root_list.update(cx, |list, _cx| list.focus(window));
                    }
                    cx.stop_propagation();
                } else if is_edit {
                    root_state.update(cx, |state, _cx| state.cancel_edit_item());
                    root_this.update(cx, |this, cx| {
                        this.switch_view("clipboard");
                        cx.notify();
                    });
                    root_list.update(cx, |list, _cx| list.focus(window));
                    cx.stop_propagation();
                } else {
                    // Clipboard view: delegate to list for panel/multi-select logic
                    let should_hide =
                        root_list.update(cx, |list, cx| !list.handle_escape_from_root(cx));
                    if should_hide {
                        root_this.update(cx, |this, cx| {
                            this.window_manager.update(cx, |wm, cx| wm.hide(cx));
                        });
                    } else {
                        // Panels were dismissed by ESC — return focus to the list
                        root_list.update(cx, |list, _cx| list.focus(window));
                    }
                    cx.stop_propagation();
                }
            })
            .child(div().absolute().left(px(0.)).top(px(65.)).child(sidebar))
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
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .overflow_hidden()
                            .opacity(view_opacity)
                            .mt(px(view_offset))
                            .when(is_clipboard, |view| {
                                let transfer_active = self.state.read(cx).transfer_filter_active;
                                view.child(search_box.clone())
                                    .when(!transfer_active, |view| view.child(filter_bar.clone()))
                                    .child(
                                        div()
                                            .relative()
                                            .flex_1()
                                            .flex()
                                            .flex_col()
                                            .overflow_hidden()
                                            .child(list_view.clone())
                                            .when(transfer_refreshing, |list| {
                                                list.child(
                                                    div()
                                                        .absolute()
                                                        .right(px(12.))
                                                        .bottom(px(12.))
                                                        .size(px(22.))
                                                        .rounded(px(11.))
                                                        .bg(theme.toast_bg)
                                                        .border(px(1.))
                                                        .border_color(theme.divider)
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .child(activity_spinner(
                                                            "transfer-refresh-spinner",
                                                            theme.accent,
                                                            17.,
                                                        )),
                                                )
                                            }),
                                    )
                            })
                            .when(is_settings, |view| view.child(settings_panel))
                            .when(is_edit, |view| view.child(edit_panel)),
                    )
                    // ── Bottom foreground-app status bar (clipboard + settings views) ──
                    .when(!is_edit, |panel| {
                        let bottom = BottomBar::new(
                            self.state.clone(),
                            self.settings_panel.clone(),
                            theme.clone(),
                        );
                        panel.child(bottom)
                    }),
            )
            // --- Tag filter panel — ConfirmDialog pattern: ---
            // --- full-screen backdrop that closes on click outside, ---
            // --- panel positioned top-right, occlude prevents click-through. ---
            .when(tag_panel_visible, |root| {
                let search_for_backdrop = filter_bar.clone();
                root.child(
                    div()
                        .absolute()
                        .left(px(36.))
                        .right(px(0.))
                        .top(px(0.))
                        .bottom(px(0.))
                        .occlude()
                        .bg(rgba(0x00000033))
                        .rounded(px(12.))
                        .opacity(tag_panel_opacity)
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            search_for_backdrop.update(cx, |bar, cx| bar.close_tag_panel(cx));
                        })
                        .child(
                            div()
                                .absolute()
                                .right(px(8.))
                                .top(px(93. - tag_panel_offset))
                                .occlude()
                                .child(tag_filter_panel),
                        ),
                )
            })
            // --- Type filter config panel — same backdrop pattern as tag filter ---
            .when(filter_config_visible, |root| {
                let search_for_backdrop = filter_bar.clone();
                let config_panel = self.type_filter_config_panel.clone();
                root.child(
                    div()
                        .absolute()
                        .left(px(36.))
                        .right(px(0.))
                        .top(px(0.))
                        .bottom(px(0.))
                        .occlude()
                        .bg(rgba(0x00000033))
                        .rounded(px(12.))
                        .opacity(filter_config_opacity)
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            search_for_backdrop.update(cx, |bar, cx| bar.close_filter_config(cx));
                        })
                        .child(
                            div()
                                .absolute()
                                .right(px(8.))
                                .top(px(93. - filter_config_offset))
                                .occlude()
                                .child(config_panel),
                        ),
                )
            })
            // --- Tag edit overlay — centered in main panel area (left:36px) ---
            .when(editing_tag_visible, |root| {
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
                        .bg(rgba(0x00000033))
                        .rounded(px(12.))
                        .opacity(edit_tag_opacity)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(render_edit_panel(
                            &edit_name_input,
                            &editing_tag_color,
                            self.theme.clone(),
                            edit_tag_scale,
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
            })
            .when(context_menu_visible, |root| {
                let list = self.list_view.clone();
                let list_for_action = self.list_view.clone();
                let (menu_x, menu_y) = list.read(cx).context_menu_position();
                let menu_y =
                    menu_y + Self::directional_overlay_offset(menu_y, win_h, context_menu_offset);
                let is_batch = list.read(cx).context_menu_is_batch();
                let is_transfer_batch = list.read(cx).context_menu_is_transfer_batch();
                let item = list.read(cx).context_menu_item().cloned();

                // --- Backdrop — click / scroll to dismiss ---
                root.child(
                    div()
                        .absolute()
                        .left(px(36.))
                        .right(px(0.))
                        .top(px(0.))
                        .bottom(px(0.))
                        .occlude()
                        .on_mouse_down(MouseButton::Left, {
                            let l = list.clone();
                            move |_ev, _window, cx| {
                                l.update(cx, |lst, cx| lst.dismiss_context_menu(cx));
                            }
                        })
                        .on_scroll_wheel({
                            let l = list.clone();
                            move |_ev, _window, cx| {
                                l.update(cx, |lst, cx| lst.dismiss_context_menu(cx));
                            }
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .opacity(context_menu_opacity)
                        .occlude()
                        .child({
                            let l = list_for_action.clone();
                            if is_transfer_batch {
                                ContextMenu::for_transfer_batch()
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
                            } else if is_batch {
                                let count = l.read(cx).selected_count;
                                let can_merge = l.read(cx).can_merge_selected_items(cx);
                                ContextMenu::for_batch(count, can_merge)
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
                                // Check if this is a transfer station entry
                                let fd =
                                    crate::core::types::FileData::from_json(&clip_item.file_data);
                                if self.state.read(cx).transfer_filter_active && fd.is_transfer() {
                                    let is_local = clip_item.tags.iter().any(|t| {
                                        t.name
                                            == crate::core::i18n_keys::I18nKey::TransferLocal.text()
                                    });
                                    let is_pinned = clip_item.tags.iter().any(|t| {
                                        t.uid
                                            == crate::core::transfer_types::TRANSFER_STATUS_PINNED_UID
                                    });
                                    ContextMenu::for_transfer_entry(is_local, is_pinned)
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
                                    let mut ctx = MenuItemContext::from_item(clip_item);
                                    ctx.transfer_enabled = self.state.read(cx).transfer_available();
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
                                }
                            } else {
                                div().into_any_element()
                            }
                        }),
                )
            })
            .when(tag_picker_visible, |root| {
                let list = self.list_view.clone();
                let list_for_panel = self.list_view.clone();
                let (picker_x, picker_y) = list.read(cx).tag_picker_position();
                let is_batch = list.read(cx).tag_picker_is_batch();
                let rows = list.update(cx, |list, cx| list.tag_picker_rows(cx));
                let create_input = list.read(cx).tag_create_input().clone();
                let clamped_x = picker_x.clamp(4.0, (win_w - 304.0 - 4.0).max(4.0));
                let base_y = picker_y.clamp(4.0, (win_h - 300.0 - 4.0).max(4.0));
                let clamped_y = base_y + tag_picker_offset;

                root.child(
                    div()
                        .absolute()
                        .left(px(36.))
                        .right(px(0.))
                        .top(px(0.))
                        .bottom(px(0.))
                        .occlude()
                        .bg(rgba(0x00000033))
                        .rounded(px(12.))
                        .opacity(tag_picker_opacity)
                        .on_mouse_down(MouseButton::Left, {
                            let l = list.clone();
                            move |_ev, _window, cx| {
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
            })
            .when(confirm_dialog_visible, |root| {
                let list = self.list_view.clone();
                let app_state = self.state.clone();
                let dialog_focus = cx.focus_handle();

                // --- Read dialog state and clone what we need before closures ---
                let dialog = list.read(cx).confirm_dialog_state().cloned();
                let dialog_element: AnyElement = match dialog {
                    Some(ConfirmDialogState::Single { id }) => ConfirmDialog::delete_single()
                        .theme(self.theme.clone())
                        .focus_handle(dialog_focus.clone())
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
                        .render_animated(window, cx, confirm_dialog_gen),
                    Some(ConfirmDialogState::Batch { count }) => ConfirmDialog::delete_batch(count)
                        .theme(self.theme.clone())
                        .focus_handle(dialog_focus.clone())
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
                        .render_animated(window, cx, confirm_dialog_gen),
                    Some(ConfirmDialogState::TransferBatch { hashes }) => {
                        ConfirmDialog::delete_batch(hashes.len())
                            .theme(self.theme.clone())
                            .focus_handle(dialog_focus.clone())
                            .on_confirm({
                                let state = app_state.clone();
                                let list = list.clone();
                                move |_window, cx| {
                                    state.update(cx, |state, _cx| {
                                        state.delete_transfer_entries(&hashes)
                                    });
                                    list.update(cx, |list, cx| {
                                        list.dismiss_confirm_dialog(cx);
                                    });
                                }
                            })
                            .on_cancel({
                                let list = list.clone();
                                move |_window, cx| {
                                    list.update(cx, |list, cx| list.dismiss_confirm_dialog(cx));
                                }
                            })
                            .render_animated(window, cx, confirm_dialog_gen)
                    }
                    Some(ConfirmDialogState::Transfer { hash }) => ConfirmDialog::delete_single()
                        .theme(self.theme.clone())
                        .focus_handle(dialog_focus.clone())
                        .on_confirm({
                            let state = app_state.clone();
                            let list = list.clone();
                            move |_window, cx| {
                                state.update(cx, |state, _cx| state.delete_transfer_entry(&hash));
                                list.update(cx, |list, cx| {
                                    list.dismiss_confirm_dialog(cx);
                                });
                            }
                        })
                        .on_cancel({
                            let list = list.clone();
                            move |_window, cx| {
                                list.update(cx, |list, cx| list.dismiss_confirm_dialog(cx));
                            }
                        })
                        .render_animated(window, cx, confirm_dialog_gen),
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
            })
            .when(latest_hotkeys_popup_visible, |root| {
                let latest_hotkeys = self.state.read(cx).settings.latest_hotkeys.clone();
                let latest_hotkeys_recording_slot =
                    self.window_manager.read(cx).recording_latest_slot();
                root.child(SettingsPanel::render_latest_hotkeys_popup_overlay(
                    self.settings_panel.clone(),
                    self.state.clone(),
                    self.window_manager.clone(),
                    latest_hotkeys,
                    latest_hotkeys_recording_slot,
                    self.theme.clone(),
                    (
                        latest_hotkeys_popup_opacity,
                        latest_hotkeys_popup_scale,
                        latest_hotkeys_popup_offset,
                        win_w,
                        win_h,
                    ),
                ))
            })
            .when(hotkey_blacklist_popup_visible, |root| {
                let settings = self.settings_panel.clone();
                let state = self.state.clone();
                let foreground_app = state.read(cx).foreground_app_name.clone();
                let entries = state
                    .read(cx)
                    .settings
                    .hotkey_blacklist
                    .iter()
                    .map(|app_name| AppListDialogEntry {
                        app_name: app_name.clone(),
                        shortcut: None,
                        is_recording_target: false,
                    })
                    .collect();
                let add_button_label = (!foreground_app.is_empty())
                    .then(|| I18nKey::ClipboardAppBlacklistAddLabel.fmt(&[&foreground_app]));
                root.child(render_app_list_dialog(AppListDialogParams {
                    title: I18nKey::HotkeyBlacklist.text().to_string(),
                    empty_hint: I18nKey::HotkeyBlacklistEmptyHint.text().to_string(),
                    entries,
                    scroll_key: "hotkey-blacklist-popup",
                    recording_app: None,
                    show_shortcut_column: false,
                    help_text: None,
                    add_button_label,
                    add_button_recording: false,
                    theme: self.theme.clone(),
                    layout: (
                        hotkey_blacklist_popup_opacity,
                        hotkey_blacklist_popup_scale,
                        hotkey_blacklist_popup_offset,
                        win_w,
                        win_h,
                    ),
                    on_close: Rc::new({
                        let settings = settings.clone();
                        move |_window, cx| {
                            settings.update(cx, |panel, cx| {
                                panel.close_app_list_popups();
                                cx.notify();
                            });
                        }
                    }),
                    on_delete: Rc::new({
                        let settings = settings.clone();
                        move |app_name, _window, cx| {
                            settings.update(cx, |_panel, cx| {
                                cx.emit(SettingsEvent::ShowHotkeyConfirm(
                                    hotkey::HotkeyConfirmAction::RemoveBlacklist { app_name },
                                ));
                            });
                        }
                    }),
                    on_shortcut_click: None,
                    on_cancel_recording: None,
                    on_add: (!foreground_app.is_empty()).then(|| {
                        Rc::new({
                            let settings = settings.clone();
                            let state = state.clone();
                            let foreground_app = foreground_app.clone();
                            move |_window: &mut Window, cx: &mut App| {
                                if crate::core::settings::is_app_in_list(
                                    &state.read(cx).settings.hotkey_blacklist,
                                    &foreground_app,
                                ) {
                                    state.update(cx, |state, cx| {
                                        state.show_toast(I18nKey::BottomBarAlreadyInList.text());
                                        cx.notify();
                                    });
                                } else {
                                    settings.update(cx, |_panel, cx| {
                                        cx.emit(SettingsEvent::ShowHotkeyConfirm(
                                            hotkey::HotkeyConfirmAction::AddBlacklist {
                                                app_name: foreground_app.clone(),
                                            },
                                        ));
                                    });
                                }
                            }
                        }) as Rc<dyn Fn(&mut Window, &mut App)>
                    }),
                }))
            })
            .when(paste_shortcuts_popup_visible, |root| {
                let settings = self.settings_panel.clone();
                let state = self.state.clone();
                let wm = self.window_manager.clone();
                let foreground_app = state.read(cx).foreground_app_name.clone();
                let recording_app = settings.read(cx).recording_paste_shortcut.clone();
                let recording_active = recording_app.is_some();
                let entries = state
                    .read(cx)
                    .settings
                    .paste_shortcuts
                    .iter()
                    .map(|entry| AppListDialogEntry {
                        app_name: entry.app_name.clone(),
                        shortcut: Some(entry.shortcut.clone()),
                        is_recording_target: recording_app
                            .as_deref()
                            .map(|app| app.eq_ignore_ascii_case(&entry.app_name))
                            .unwrap_or(false),
                    })
                    .collect();
                let add_button_label = if recording_active {
                    Some(I18nKey::HotkeyPasteShortcutRecording.text().to_string())
                } else {
                    (!foreground_app.is_empty())
                        .then(|| I18nKey::ClipboardAppBlacklistAddLabel.fmt(&[&foreground_app]))
                };
                root.child(render_app_list_dialog(AppListDialogParams {
                    title: I18nKey::HotkeyPasteShortcut.text().to_string(),
                    empty_hint: I18nKey::HotkeyPasteShortcutEmptyHint.text().to_string(),
                    entries,
                    scroll_key: "paste-shortcuts-popup",
                    recording_app,
                    show_shortcut_column: true,
                    help_text: Some(I18nKey::HotkeyPasteShortcutHelp.text().to_string()),
                    add_button_label,
                    add_button_recording: recording_active,
                    theme: self.theme.clone(),
                    layout: (
                        paste_shortcuts_popup_opacity,
                        paste_shortcuts_popup_scale,
                        paste_shortcuts_popup_offset,
                        win_w,
                        win_h,
                    ),
                    on_close: Rc::new({
                        let settings = settings.clone();
                        let wm = wm.clone();
                        move |_window, cx| {
                            wm.update(cx, |wm, _cx| {
                                wm.cancel_paste_shortcut_recording();
                            });
                            settings.update(cx, |panel, cx| {
                                panel.clear_paste_shortcut_state(cx);
                                panel.close_app_list_popups();
                            });
                        }
                    }),
                    on_delete: Rc::new({
                        let settings = settings.clone();
                        move |app_name, _window, cx| {
                            settings.update(cx, |_panel, cx| {
                                cx.emit(SettingsEvent::ShowHotkeyConfirm(
                                    hotkey::HotkeyConfirmAction::RemovePasteShortcut { app_name },
                                ));
                            });
                        }
                    }),
                    on_shortcut_click: Some(Rc::new({
                        let settings = settings.clone();
                        let wm = wm.clone();
                        move |app_name, _window, cx| {
                            settings.update(cx, |panel, cx| {
                                panel.recording_paste_shortcut = Some(app_name.clone());
                                cx.notify();
                            });
                            wm.update(cx, |wm, cx| {
                                wm.start_paste_shortcut_recording(app_name, cx);
                            });
                        }
                    })),
                    on_cancel_recording: Some(Rc::new({
                        let settings = settings.clone();
                        let wm = wm.clone();
                        move |_window, cx| {
                            wm.update(cx, |wm, _cx| wm.cancel_paste_shortcut_recording());
                            settings.update(cx, |panel, cx| {
                                panel.clear_paste_shortcut_state(cx);
                            });
                        }
                    })),
                    on_add: (recording_active || !foreground_app.is_empty()).then(|| {
                        Rc::new({
                            let settings = settings.clone();
                            let wm = wm.clone();
                            let foreground_app = foreground_app.clone();
                            move |_window: &mut Window, cx: &mut App| {
                                if recording_active {
                                    wm.update(cx, |wm, _cx| {
                                        wm.cancel_paste_shortcut_recording();
                                    });
                                    settings.update(cx, |panel, cx| {
                                        panel.clear_paste_shortcut_state(cx);
                                    });
                                } else {
                                    settings.update(cx, |panel, cx| {
                                        panel.recording_paste_shortcut =
                                            Some(foreground_app.clone());
                                        cx.notify();
                                    });
                                    wm.update(cx, |wm, cx| {
                                        wm.start_paste_shortcut_recording(
                                            foreground_app.clone(),
                                            cx,
                                        );
                                    });
                                }
                            }
                        }) as Rc<dyn Fn(&mut Window, &mut App)>
                    }),
                }))
            })
            .when(app_blacklist_popup_visible, |root| {
                let settings = self.settings_panel.clone();
                let state = self.state.clone();
                let foreground_app = state.read(cx).foreground_app_name.clone();
                let entries = state
                    .read(cx)
                    .settings
                    .clipboard_app_blacklist
                    .iter()
                    .map(|app_name| AppListDialogEntry {
                        app_name: app_name.clone(),
                        shortcut: None,
                        is_recording_target: false,
                    })
                    .collect();
                let add_button_label = (!foreground_app.is_empty())
                    .then(|| I18nKey::ClipboardAppBlacklistAddLabel.fmt(&[&foreground_app]));
                root.child(render_app_list_dialog(AppListDialogParams {
                    title: I18nKey::ClipboardAppBlacklistTitle.text().to_string(),
                    empty_hint: I18nKey::ClipboardAppBlacklistEmptyHint.text().to_string(),
                    entries,
                    scroll_key: "app-blacklist-popup",
                    recording_app: None,
                    show_shortcut_column: false,
                    help_text: None,
                    add_button_label,
                    add_button_recording: false,
                    theme: self.theme.clone(),
                    layout: (
                        app_blacklist_popup_opacity,
                        app_blacklist_popup_scale,
                        app_blacklist_popup_offset,
                        win_w,
                        win_h,
                    ),
                    on_close: Rc::new({
                        let settings = settings.clone();
                        move |_window, cx| {
                            settings.update(cx, |panel, cx| {
                                panel.close_app_list_popups();
                                cx.notify();
                            })
                        }
                    }),
                    on_delete: Rc::new({
                        let settings = settings.clone();
                        move |app_name, _window, cx| {
                            settings.update(cx, |_panel, cx| {
                                cx.emit(SettingsEvent::ShowHotkeyConfirm(
                                    hotkey::HotkeyConfirmAction::RemoveClipboardBlacklist {
                                        app_name,
                                    },
                                ))
                            })
                        }
                    }),
                    on_shortcut_click: None,
                    on_cancel_recording: None,
                    on_add: (!foreground_app.is_empty()).then(|| {
                        Rc::new({
                            let settings = settings.clone();
                            let state = state.clone();
                            let foreground_app = foreground_app.clone();
                            move |_window: &mut Window, cx: &mut App| {
                                if crate::core::settings::is_app_in_list(
                                    &state.read(cx).settings.clipboard_app_blacklist,
                                    &foreground_app,
                                ) {
                                    state.update(cx, |state, cx| {
                                        state.show_toast(I18nKey::BottomBarAlreadyInList.text());
                                        cx.notify();
                                    });
                                } else {
                                    settings.update(cx, |_panel, cx| {
                                        cx.emit(SettingsEvent::ShowHotkeyConfirm(
                                            hotkey::HotkeyConfirmAction::AddClipboardBlacklist {
                                                app_name: foreground_app.clone(),
                                            },
                                        ))
                                    });
                                }
                            }
                        }) as Rc<dyn Fn(&mut Window, &mut App)>
                    }),
                }))
            })
            // --- Root-level clear-data ConfirmDialog ---
            .when(clear_data_confirm_visible, |root| {
                let include_favorites = self.clear_data_include_favorites;
                let app_state = self.state.clone();
                let items_count = if include_favorites {
                    app_state.read(cx).clearable_history_count()
                } else {
                    app_state.read(cx).clearable_non_favorite_history_count()
                };
                let has_enabled_sync = app_state
                    .read(cx)
                    .settings
                    .sync_backends
                    .iter()
                    .any(|backend| backend.enabled);
                let mut message = I18nKey::ConfirmClearDataMsg.fmt(&[&items_count.to_string()]);
                if has_enabled_sync {
                    message.push('\n');
                    message.push_str(I18nKey::ConfirmClearDataSyncWarning.text());
                }

                let wm = self.window_manager.clone();
                let dialog_focus = cx.focus_handle();
                root.child(
                    div()
                        .absolute()
                        .left(px(36.))
                        .right(px(0.))
                        .top(px(0.))
                        .bottom(px(0.))
                        .child(
                            ConfirmDialog::new()
                                .title(I18nKey::ConfirmClearDataTitle.text())
                                .message(message)
                                .option(
                                    I18nKey::ConfirmClearDataIncludeFavorites.text(),
                                    include_favorites,
                                    {
                                        let root_entity = root_entity.clone();
                                        move |_window, cx| {
                                            root_entity.update(cx, |root, cx| {
                                                root.clear_data_include_favorites =
                                                    !root.clear_data_include_favorites;
                                                cx.notify();
                                            });
                                        }
                                    },
                                )
                                .confirm_label(I18nKey::BtnClearData.text())
                                .danger(true)
                                .theme(self.theme.clone())
                                .on_confirm({
                                    let root_entity = root_entity.clone();
                                    let app_state = app_state.clone();
                                    move |_window, cx| {
                                        let accepted = wm.update(cx, |wm, cx| {
                                            wm.request_clear_data(include_favorites, cx)
                                        });
                                        root_entity.update(cx, |root, cx| {
                                            root.clear_data_confirm = false;
                                            if !accepted {
                                                app_state.update(cx, |state, _cx| {
                                                    state.toast_message = Some(
                                                        I18nKey::BtnCleaning.text().to_string(),
                                                    );
                                                });
                                            }
                                            cx.notify();
                                        });
                                    }
                                })
                                .on_cancel({
                                    let root_entity = root_entity.clone();
                                    move |_window, cx| {
                                        root_entity.update(cx, |root, cx| {
                                            root.clear_data_confirm = false;
                                            cx.notify();
                                        });
                                    }
                                })
                                .focus_handle(dialog_focus)
                                .render_animated(window, cx, clear_data_confirm_gen),
                        ),
                )
            })
            // --- Settings hotkey blacklist ConfirmDialog ---
            .when(hotkey_confirm_visible, |root| {
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
                            .render_animated(window, cx, hotkey_confirm_gen)
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
                            .render_animated(window, cx, hotkey_confirm_gen)
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
                            .render_animated(window, cx, hotkey_confirm_gen)
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
                                            action:
                                                hotkey::HotkeyConfirmAction::RemovePasteShortcut {
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
                            .render_animated(window, cx, hotkey_confirm_gen)
                    }
                    // ── Clipboard app blacklist ──
                    Some(hotkey::HotkeyConfirmAction::AddClipboardBlacklist { app_name }) => {
                        ConfirmDialog::add_clipboard_blacklist(&app_name)
                            .theme(self.theme.clone())
                            .on_confirm({
                                let wm = wm.clone();
                                let app_state = app_state.clone();
                                let settings = settings.clone();
                                let app_name = app_name.clone();
                                move |_window, cx| {
                                    app_state.update(cx, |s, _cx| {
                                        if !crate::core::settings::is_app_in_list(
                                            &s.settings.clipboard_app_blacklist,
                                            &app_name,
                                        ) {
                                            s.settings
                                                .clipboard_app_blacklist
                                                .push(app_name.clone());
                                            s.settings.save();
                                        }
                                    });
                                    let updated =
                                        app_state.read(cx).settings.clipboard_app_blacklist.clone();
                                    wm.update(cx, |wm, _cx| {
                                        wm.set_clipboard_app_blacklist(updated);
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
                            .render_animated(window, cx, hotkey_confirm_gen)
                    }
                    Some(hotkey::HotkeyConfirmAction::RemoveClipboardBlacklist { app_name }) => {
                        ConfirmDialog::remove_clipboard_blacklist(&app_name)
                            .theme(self.theme.clone())
                            .on_confirm({
                                let wm = wm.clone();
                                let app_state = app_state.clone();
                                let settings = settings.clone();
                                let app_name = app_name.clone();
                                move |_window, cx| {
                                    app_state.update(cx, |s, _cx| {
                                        s.settings.clipboard_app_blacklist.retain(|a| {
                                            !crate::core::settings::is_app_in_list(
                                                std::slice::from_ref(a),
                                                &app_name,
                                            )
                                        });
                                        s.settings.save();
                                    });
                                    let updated =
                                        app_state.read(cx).settings.clipboard_app_blacklist.clone();
                                    wm.update(cx, |wm, _cx| {
                                        wm.set_clipboard_app_blacklist(updated);
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
                            .render_animated(window, cx, hotkey_confirm_gen)
                    }
                    // ── Win+V takeover ──
                    Some(hotkey::HotkeyConfirmAction::WinVTakeoverEnable) => {
                        let fallback_hotkey = app_state.read(cx).settings.hotkey.clone();
                        ConfirmDialog::new()
                            .title(I18nKey::WinVTakeoverConfirmTitle.text())
                            .message(I18nKey::WinVTakeoverConfirmMsg.fmt(&[&fallback_hotkey]))
                            .confirm_label(I18nKey::DialogConfirm.text())
                            .theme(self.theme.clone())
                            .on_confirm({
                                let wm = wm.clone();
                                let settings = settings.clone();
                                let app_state = app_state.clone();
                                move |_window, cx| {
                                    let result =
                                        wm.update(cx, |wm, cx| wm.enable_win_v_takeover(cx));
                                    if let Err(e) = result {
                                        log::error!("Win+V takeover enable failed: {e}");
                                        app_state.update(cx, |state, _cx| {
                                            state.toast_message = Some(e);
                                            state.toast_is_warning = true;
                                        });
                                    }
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
                            .render_animated(window, cx, hotkey_confirm_gen)
                    }
                    Some(hotkey::HotkeyConfirmAction::WinVTakeoverDisable) => ConfirmDialog::new()
                        .title(I18nKey::WinVTakeoverRestoreTitle.text())
                        .message(I18nKey::WinVTakeoverRestoreMsg.text())
                        .confirm_label(I18nKey::DialogConfirm.text())
                        .theme(self.theme.clone())
                        .on_confirm({
                            let wm = wm.clone();
                            let settings = settings.clone();
                            let app_state = app_state.clone();
                            move |_window, cx| {
                                let result = wm.update(cx, |wm, cx| wm.disable_win_v_takeover(cx));
                                if let Err(e) = result {
                                    log::error!("Win+V takeover disable failed: {e}");
                                    app_state.update(cx, |state, _cx| {
                                        state.toast_message = Some(e);
                                        state.toast_is_warning = true;
                                    });
                                }
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
                        .render_animated(window, cx, hotkey_confirm_gen),
                    // ── Win+V manual setup instructions ──
                    Some(hotkey::HotkeyConfirmAction::WinVManualSetup) => ConfirmDialog::new()
                        .title(I18nKey::WinVManualSetupHint.text())
                        .message(I18nKey::WinVManualSetupContent.text())
                        .confirm_label(I18nKey::DialogConfirm.text())
                        .theme(self.theme.clone())
                        .on_confirm({
                            let settings = settings.clone();
                            move |_window, cx| {
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
                        .render_animated(window, cx, hotkey_confirm_gen),
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
            })
            .when(backend_panel_visible, |root| {
                root.child(
                    div()
                        .absolute()
                        .left(px(36.))
                        .right(px(0.))
                        .top(px(0.))
                        .bottom(px(0.))
                        .opacity(backend_panel_opacity)
                        .child(backend_panel),
                )
            })
            .when(
                {
                    let toast_visible = self.state.read(cx).toast_message.is_some();
                    // --- Keep the toast rendered during the exit animation so it can fade out. ---
                    // Show toast regardless of active view so recording conflict /
                    // fallback messages are visible from the settings panel.
                    toast_visible || self.toast_dismissing
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
                                        el.cursor(CursorStyle::PointingHand).on_mouse_down(
                                            MouseButton::Left,
                                            move |_ev, _window, cx| {
                                                state.update(cx, |s, _cx| s.clear_toast());
                                            },
                                        )
                                    })
                                    .when(has_actions, |el| el.occlude())
                                    .when(dismiss_state, |el| el.cursor(CursorStyle::Arrow))
                                    .child({
                                        let kind = if self.state.read(cx).toast_is_warning {
                                            super::components::toast::ToastKind::Warn
                                        } else {
                                            super::components::toast::ToastKind::Info
                                        };
                                        Toast::new(message).theme(theme).actions(actions).kind(kind)
                                    }),
                            ),
                    )
                },
            )
    }
}

impl RootView {
    /// Unified transfer-station toggle handled here (not the titlebar):
    /// closes list overlays, switches view state, and restores the page the
    /// station was opened from. Plain list/settings switches never change the
    /// transfer state or the recorded return page.
    fn handle_toggle_transfer(&mut self, cx: &mut Context<Self>) {
        // Dismiss floating list overlays before switching modes.
        self.filter_bar.update(cx, |bar, cx| {
            bar.close_tag_panel(cx);
            bar.close_filter_config(cx);
        });
        self.list_view.update(cx, |list, cx| {
            list.dismiss_context_menu(cx);
            list.hide_tag_picker(cx);
        });

        let current_view = self.current_view.clone();
        let transfer_active = self.state.read(cx).transfer_filter_active;
        let return_view = self.transfer_return_view.clone();
        match (
            current_view.as_str(),
            transfer_active,
            return_view.as_deref(),
        ) {
            // List page, station off: open it and stay on the list.
            ("clipboard", false, _) => {
                self.state
                    .update(cx, |state, _cx| state.toggle_transfer_filter());
                self.transfer_return_view = None;
            }
            // Settings page, station off: remember the return page, switch to
            // the list and open the station there.
            ("settings", false, _) => {
                self.transfer_return_view = Some("settings".to_string());
                self.state
                    .update(cx, |state, _cx| state.toggle_transfer_filter());
                self.switch_view("clipboard");
            }
            // List page with a recorded return page: close and go back.
            ("clipboard", true, Some(return_view)) => {
                self.state
                    .update(cx, |state, _cx| state.toggle_transfer_filter());
                self.switch_view(return_view);
                self.transfer_return_view = None;
            }
            // List page, station on: close it and stay.
            ("clipboard", true, None) => {
                self.state
                    .update(cx, |state, _cx| state.toggle_transfer_filter());
            }
            // Settings page with the station still on: close and stay.
            ("settings", true, _) => {
                self.state
                    .update(cx, |state, _cx| state.toggle_transfer_filter());
                self.transfer_return_view = None;
            }
            _ => {}
        }

        let items = self.state.read(cx).visible_items();
        self.list_view
            .update(cx, |list, cx| list.set_items(items, cx));
        self.titlebar.update(cx, |_titlebar, cx| cx.notify());
        cx.notify();
    }

    fn switch_view(&mut self, view: &str) {
        if self.current_view != view {
            self.current_view = view.into();
            self.view_transition_generation = self.view_transition_generation.wrapping_add(1);
            self.view_transition_started = Some(Instant::now());
        }
    }

    fn overlay_generation(&mut self, key: &'static str, visible: bool) -> u64 {
        let state = self.overlay_transitions.entry(key).or_default();
        if visible && !state.visible {
            state.generation = state.generation.wrapping_add(1);
            state.started_at = Some(Instant::now());
        } else if !visible {
            state.started_at = None;
        }
        state.visible = visible;
        state.generation
    }

    fn overlay_animating(&self, key: &'static str) -> bool {
        self.overlay_transitions
            .get(key)
            .and_then(|state| state.started_at)
            .is_some_and(|started_at| {
                Self::animation_running(Some(started_at), OVERLAY_ANIM_DURATION)
            })
    }

    fn animation_running(started_at: Option<Instant>, duration: Duration) -> bool {
        started_at
            .is_some_and(|started_at| started_at.elapsed() <= duration + Duration::from_millis(24))
    }

    fn overlay_opacity(
        window: &mut Window,
        cx: &mut Context<Self>,
        generation: u64,
        key: &'static str,
    ) -> f32 {
        Self::transition_f32(
            window,
            cx,
            (key, generation.wrapping_add(10_000)),
            OVERLAY_ANIM_DURATION,
            0.0,
            1.0,
        )
    }

    fn overlay_offset(
        window: &mut Window,
        cx: &mut Context<Self>,
        generation: u64,
        key: &'static str,
    ) -> f32 {
        Self::transition_f32(
            window,
            cx,
            (key, generation.wrapping_add(20_000)),
            OVERLAY_ANIM_DURATION,
            5.0,
            0.0,
        )
    }

    fn overlay_scale(
        window: &mut Window,
        cx: &mut Context<Self>,
        generation: u64,
        key: &'static str,
    ) -> f32 {
        Self::transition_f32(
            window,
            cx,
            (key, generation.wrapping_add(30_000)),
            OVERLAY_ANIM_DURATION,
            0.96,
            1.0,
        )
    }

    fn directional_overlay_offset(anchor_y: f32, container_height: f32, offset: f32) -> f32 {
        if anchor_y < container_height * 0.5 {
            -offset
        } else {
            offset
        }
    }

    fn transition_f32(
        window: &mut Window,
        cx: &mut Context<Self>,
        key: (&'static str, u64),
        duration: Duration,
        initial: f32,
        target: f32,
    ) -> f32 {
        let transition = window
            .use_keyed_transition(key, cx, duration, move |_, _| initial)
            .with_easing(RootView::ease_out);
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

    fn toast_ease_out(_delta: f32) -> f32 {
        1.0 - (1.0 - _delta).powi(3)
    }
}
