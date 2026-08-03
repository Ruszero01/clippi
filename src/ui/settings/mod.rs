//! --- Settings panel — scrollable settings UI with tabs. ---
//!
//! --- Matches the original Slint `SettingsPanel.slint` layout: ---
//! --- - Top navigation bar: back button (→ icon) + "Settings" title (36px) ---
//! --- - Tab bar: 5 equal-width tabs (General/Clipboard/Hotkey/Data/Sync) ---
//!   with accent-green underline for active tab
//! --- - Scrollable content area routed by active tab index ---
//!
//! --- Individual settings controls will be added in follow-up work. ---
//! --- Tab rendering methods (`render_*_tab`) serve as extension points. ---

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::InputState;
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use gpui_transitions::WindowUseTransition;

mod clipboard;
mod data;
mod general;
pub mod hotkey;
mod sync;
mod version;

const TAB_ANIM_DURATION: Duration = Duration::from_millis(160);

use data::ResetDataDirState;
use hotkey::HotkeyConfirmAction;

use crate::core::i18n_keys::I18nKey;
use crate::state::app::AppState;
use crate::ui::add_backend::AddBackendPanel;
use crate::ui::components::confirm_dialog::ConfirmDialog;
use crate::ui::components::toggle::{render_toggle, ToggleColors, ToggleTransitionState};
use crate::ui::theme::ClippiTheme;
use crate::ui::window_manager::WindowManager;

/// Events emitted by the settings panel.
pub enum SettingsEvent {
    /// User clicked the back button — return to clipboard view.
    Back,
    /// Active tab changed — RootView should clear update toast when
    /// switching to the version tab (index 5).
    TabChanged(usize),
    /// Theme setting changed — RootView should rebuild its ClippiTheme.
    ThemeChanged(String),
    ClipboardSettingsChanged {
        reload_items: bool,
        scroll_to_top: bool,
    },
    /// User clicked add/remove blacklist — RootView should show a ConfirmDialog.
    ShowHotkeyConfirm(HotkeyConfirmAction),
    /// User confirmed add/remove paste shortcut — RootView should apply changes.
    HotkeyPasteShortcut { action: HotkeyConfirmAction },
    /// Data settings error — RootView should show a toast.
    DataError(String),
    /// Data settings info toast — no error prefix.
    DataToast(String),
    /// Show the clear-data confirmation at the RootView overlay level.
    ShowClearDataConfirm,
}

impl EventEmitter<SettingsEvent> for SettingsPanel {}

/// The settings panel entity.
pub struct SettingsPanel {
    active_tab: usize,
    state: Entity<AppState>,
    window_manager: Entity<WindowManager>,
    theme: ClippiTheme,
    scroll_handle: ScrollHandle,
    /// Track toggle values + generation counter for transition animation.
    toggle_states: HashMap<String, ToggleTransitionState>,
    backend_collapse_states: HashMap<String, BackendCollapseState>,
    tab_transition_generation: u64,
    tab_transition_started: Option<Instant>,
    backend_panel: Entity<AddBackendPanel>,
    /// Whether the latest hotkeys popup is open.
    pub latest_hotkeys_popup_open: bool,
    /// Application list popup visibility (hotkey blacklist / paste shortcuts / clipboard blacklist).
    pub hotkey_blacklist_popup_open: bool,
    pub paste_shortcuts_popup_open: bool,
    pub app_blacklist_popup_open: bool,
    /// Pending hotkey blacklist confirmation (consumed by RootView).
    pub hotkey_confirm: Option<HotkeyConfirmAction>,
    /// Whether we are currently recording a paste shortcut for an app (Some(app_name)).
    pub recording_paste_shortcut: Option<String>,
    /// The recorded paste shortcut string before confirmation (app_name, shortcut).
    pub pending_paste_shortcut: Option<(String, String)>,
    /// Reset-data-directory dialog state (portable mode only).
    pub reset_data_dialog: Option<ResetDataDirState>,
    /// Pending backend deletion confirmation (backend id).
    pub delete_backend_confirm: Option<String>,
    delete_backend_confirm_gen: u64,
    delete_backend_confirm_started: Option<Instant>,
    /// Whether the max-items field is in editing mode.
    editing_max_items: bool,
    /// Input entity for the max-items editor (created once in constructor).
    max_items_input: Entity<InputState>,
    /// Pending async file dialog for changing the database path.
    _db_path_dialog_task: Option<Task<()>>,
    /// Focus-out subscription for the max-items input (auto-save on blur).
    _max_items_focus_sub: gpui::Subscription,
    /// Whether the retention-days field is in editing mode.
    editing_retention_days: bool,
    /// Input entity for the retention-days editor (created once in constructor).
    retention_days_input: Entity<InputState>,
    /// Focus-out subscription for the retention-days input (auto-save on blur).
    _retention_days_focus_sub: gpui::Subscription,
    /// Animation generation counter for copy-sound card expand/collapse.
    pub copy_sound_anim_gen: u64,
    /// Focus handle for keyboard events (ESC to go back).
    focus_handle: FocusHandle,

    // ── Config sync ──
    /// Selected backend ID for config sync (defaults to first available).
    pub config_sync_backend_id: Option<String>,
    /// Upload confirmation dialog state.
    pub config_sync_upload_confirm: Option<String>,
    /// Apply confirmation dialog generation (incremented on each new dialog).
    pub config_sync_apply_confirm_gen: u64,
    /// When the apply confirmation was shown.
    #[allow(dead_code)]
    pub config_sync_apply_confirm_started: Option<Instant>,
    /// Whether the config-sync backend selector dropdown is open.
    pub config_sync_menu_open: bool,
}

fn tab_names() -> [&'static str; 6] {
    [
        I18nKey::TabGeneral.text(),
        I18nKey::TabClipboard.text(),
        I18nKey::TabHotkey.text(),
        I18nKey::TabData.text(),
        I18nKey::TabSync.text(),
        I18nKey::TabVersion.text(),
    ]
}

#[derive(Clone, Copy)]
pub(crate) struct BackendCollapseState {
    pub enabled: bool,
    pub generation: u64,
}

impl SettingsPanel {
    pub fn new(
        state: Entity<AppState>,
        window_manager: Entity<WindowManager>,
        theme: ClippiTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let max_items_input = cx.new(|cx| gpui_component::input::InputState::new(window, cx));
        let retention_days_input = cx.new(|cx| gpui_component::input::InputState::new(window, cx));
        let backend_panel =
            cx.new(|cx| AddBackendPanel::new(window_manager.clone(), theme.clone(), window, cx));

        // --- Subscribe to focus-out on the max-items InputState. ---
        // --- When the input loses focus, save and exit editing. ---
        let state_sub = state.clone();
        let input_sub = max_items_input.clone();
        let handle = max_items_input.read(cx).focus_handle(cx);
        let _max_items_focus_sub =
            cx.on_focus_out(&handle, window, move |this, _ev, _window, cx| {
                if this.editing_max_items {
                    let text = input_sub.read(cx).value().to_string();
                    let n: u32 = text.trim().parse().unwrap_or(0);
                    state_sub.update(cx, |s, _cx| {
                        s.settings.max_items = n;
                        s.settings.save();
                    });
                    this.editing_max_items = false;
                    cx.notify();
                }
            });

        // --- Subscribe to focus-out on the retention-days InputState. ---
        let state_sub_rd = state.clone();
        let input_sub_rd = retention_days_input.clone();
        let handle_rd = retention_days_input.read(cx).focus_handle(cx);
        let _retention_days_focus_sub =
            cx.on_focus_out(&handle_rd, window, move |this, _ev, _window, cx| {
                if this.editing_retention_days {
                    let text = input_sub_rd.read(cx).value().to_string();
                    let n: u32 = text.trim().parse().unwrap_or(0);
                    state_sub_rd.update(cx, |s, _cx| {
                        s.settings.retention_days = n;
                        s.settings.save();
                    });
                    this.editing_retention_days = false;
                    cx.notify();
                }
            });

        Self {
            active_tab: 0,
            state,
            window_manager,
            theme,
            scroll_handle: ScrollHandle::default(),
            toggle_states: HashMap::new(),
            backend_collapse_states: HashMap::new(),
            tab_transition_generation: 0,
            tab_transition_started: None,
            focus_handle: cx.focus_handle(),
            backend_panel,
            latest_hotkeys_popup_open: false,
            hotkey_blacklist_popup_open: false,
            paste_shortcuts_popup_open: false,
            app_blacklist_popup_open: false,
            hotkey_confirm: None,
            recording_paste_shortcut: None,
            pending_paste_shortcut: None,
            reset_data_dialog: None,
            delete_backend_confirm: None,
            delete_backend_confirm_gen: 0,
            delete_backend_confirm_started: None,
            editing_max_items: false,
            max_items_input,
            _db_path_dialog_task: None,
            _max_items_focus_sub,
            editing_retention_days: false,
            retention_days_input,
            _retention_days_focus_sub,
            copy_sound_anim_gen: 0,
            config_sync_backend_id: None,
            config_sync_upload_confirm: None,
            config_sync_apply_confirm_gen: 0,
            config_sync_apply_confirm_started: None,
            config_sync_menu_open: false,
        }
    }

    /// Reload theme from the computed ClippiTheme (called by RootView after ThemeChanged).
    pub fn reload_theme(&mut self, theme: ClippiTheme, cx: &mut Context<Self>) {
        self.theme = theme.clone();
        self.backend_panel
            .update(cx, |panel, cx| panel.set_theme(theme, cx));
        cx.notify();
    }

    /// Switch to a specific tab by index.
    pub fn set_active_tab(&mut self, index: usize) {
        if self.active_tab != index {
            self.active_tab = index;
            self.tab_transition_generation = self.tab_transition_generation.wrapping_add(1);
            self.tab_transition_started = Some(Instant::now());
        }
    }

    pub fn active_tab(&self) -> usize {
        self.active_tab
    }

    pub fn close_app_list_popups(&mut self) {
        self.hotkey_blacklist_popup_open = false;
        self.paste_shortcuts_popup_open = false;
        self.app_blacklist_popup_open = false;
    }

    pub fn toggle_hotkey_blacklist_popup(&mut self) {
        let open = !self.hotkey_blacklist_popup_open;
        self.close_app_list_popups();
        self.latest_hotkeys_popup_open = false;
        self.hotkey_blacklist_popup_open = open;
    }

    pub fn toggle_paste_shortcuts_popup(&mut self) {
        let open = !self.paste_shortcuts_popup_open;
        self.close_app_list_popups();
        self.latest_hotkeys_popup_open = false;
        self.paste_shortcuts_popup_open = open;
    }

    pub fn toggle_app_blacklist_popup(&mut self) {
        let open = !self.app_blacklist_popup_open;
        self.close_app_list_popups();
        self.latest_hotkeys_popup_open = false;
        self.app_blacklist_popup_open = open;
    }

    pub fn backend_panel(&self) -> Entity<AddBackendPanel> {
        self.backend_panel.clone()
    }

    /// Render config-sync confirmation dialogs as absolute-positioned overlays
    /// on the settings panel root. This avoids clipping from the scroll container.
    fn render_config_sync_dialogs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let wm = self.window_manager.clone();
        let this = cx.entity().clone();
        let theme = self.theme.clone();

        let show_upload = self.config_sync_upload_confirm.is_some();
        let show_apply = wm.read(cx).config_sync_pending_snapshot().is_some();

        div()
            .when(show_upload || show_apply, |root| {
                root.absolute().size_full().top_0().left_0()
            })
            .when(show_upload, |root| {
                let backend_name = self.config_sync_upload_confirm.clone().unwrap_or_default();
                let msg = I18nKey::ConfigSyncConfirmUploadMsg.fmt(&[&backend_name]);
                let backend_id = self.config_sync_backend_id.clone();
                let wm = wm.clone();
                let this = this.clone();
                let theme = theme.clone();

                root.child(
                    div()
                        .absolute()
                        .size_full()
                        .top_0()
                        .left_0()
                        .bg(rgba(0x00000055))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            ConfirmDialog::new()
                                .title(I18nKey::ConfigSyncConfirmUploadTitle.text())
                                .message(msg)
                                .confirm_label(I18nKey::BtnApply.text())
                                .danger(false)
                                .theme(theme)
                                .on_confirm({
                                    let wm = wm.clone();
                                    let this = this.clone();
                                    let bid = backend_id.clone();
                                    move |_window, app| {
                                        let backends = this.read(app).state.read(app).settings.sync_backends.clone();
                                        if let Some(ref id) = bid {
                                            if let Some(cfg) = backends.iter().find(|b| &b.id == id) {
                                                if let Some(backend) = crate::services::backends::create_config_snapshot_backend(cfg) {
                                                    wm.update(app, |wm, cx| {
                                                        wm.start_config_upload(backend, cx);
                                                    });
                                                }
                                            }
                                        }
                                        this.update(app, |panel, cx| {
                                            panel.config_sync_upload_confirm = None;
                                            cx.notify();
                                        });
                                    }
                                })
                                .on_cancel({
                                    let this = this.clone();
                                    move |_window, app| {
                                        this.update(app, |panel, cx| {
                                            panel.config_sync_upload_confirm = None;
                                            cx.notify();
                                        });
                                    }
                                })
                                .render_animated(window, cx, 0),
                        ),
                )
            })
            .when(show_apply, |root| {
                let wm = wm.clone();
                let this = this.clone();
                let theme = theme.clone();
                let snapshot = wm.read(cx).config_sync_pending_snapshot().cloned();

                if let Some(ref snap) = snapshot {
                    let uploaded = format_uploaded_at(&snap.uploaded_at);
                    let platform = snap.source.platform.clone();
                    let version = snap.source.app_version.clone();
                    let msg = I18nKey::ConfigSyncConfirmApplyMsg.fmt(&[&uploaded, &platform, &version]);
                    let gen = self.config_sync_apply_confirm_gen;

                    root.child(
                        div()
                            .absolute()
                            .size_full()
                            .top_0()
                            .left_0()
                            .bg(rgba(0x00000055))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                ConfirmDialog::new()
                                    .title(I18nKey::ConfigSyncConfirmApplyTitle.text())
                                    .message(msg)
                                    .confirm_label(I18nKey::BtnRestartNow.text())
                                    .danger(false)
                                    .theme(theme)
                                    .on_confirm({
                                        let wm = wm.clone();
                                        move |_window, app| {
                                            wm.update(app, |wm, cx| {
                                                wm.apply_config_snapshot(cx);
                                            });
                                        }
                                    })
                                    .on_cancel({
                                        let wm = wm.clone();
                                        let this = this.clone();
                                        move |_window, app| {
                                            wm.update(app, |wm, _cx| {
                                                wm.clear_config_sync_pending_snapshot();
                                            });
                                            this.update(app, |_panel, cx| {
                                                cx.notify();
                                            });
                                        }
                                    })
                                    .render_animated(window, cx, gen),
                            ),
                    )
                } else {
                    root.child(div())
                }
            })
    }

    /// Render delete-backend confirmation dialog as an absolute overlay.
    fn render_delete_backend_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let show = self.delete_backend_confirm.is_some();
        let gen = if show {
            self.delete_backend_confirm_gen
        } else {
            0
        };
        let wm = self.window_manager.clone();
        let this = cx.entity().clone();
        let theme = self.theme.clone();

        div().when(show, |root| {
            let id = self.delete_backend_confirm.clone().unwrap_or_default();
            root.absolute().size_full().top_0().left_0().child(
                div()
                    .absolute()
                    .size_full()
                    .top_0()
                    .left_0()
                    .bg(rgba(0x00000055))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        ConfirmDialog::new()
                            .title(I18nKey::ConfirmDeleteSingleTitle.text())
                            .message(I18nKey::BackendDeleteConfirmMsg.text())
                            .confirm_label(I18nKey::ConfirmDeleteLabel.text())
                            .danger(true)
                            .theme(theme)
                            .on_confirm({
                                let wm = wm.clone();
                                let this = this.clone();
                                move |_window, app| {
                                    wm.update(app, |wm, cx| {
                                        wm.remove_sync_backend(&id, cx);
                                    });
                                    this.update(app, |panel, cx| {
                                        panel.delete_backend_confirm = None;
                                        cx.notify();
                                    });
                                }
                            })
                            .on_cancel({
                                let this = this.clone();
                                move |_window, app| {
                                    this.update(app, |panel, cx| {
                                        panel.delete_backend_confirm = None;
                                        cx.notify();
                                    });
                                }
                            })
                            .render_animated(window, cx, gen),
                    ),
            )
        })
    }
}

/// Format an RFC3339 timestamp as local date-time.
fn format_uploaded_at(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| rfc3339.to_string())
}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_tab;
        let theme = &self.theme;
        let this = cx.entity().clone();
        let tab_key = ((active as u64) << 32).wrapping_add(self.tab_transition_generation);
        let tab_animating = Self::animation_running(self.tab_transition_started);
        let tab_opacity = if tab_animating {
            Self::transition_f32(window, cx, ("settings-tab-opacity", tab_key), 0.0, 1.0)
        } else {
            1.0
        };
        let tab_offset = if tab_animating {
            Self::transition_f32(window, cx, ("settings-tab-offset", tab_key), 4.0, 0.0)
        } else {
            0.0
        };

        let focus_handle = self.focus_handle.clone();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .w_full()
            .overflow_hidden()
            .rounded_b(px(12.))
            .bg(theme.bg)
            .track_focus(&focus_handle)
            //  Navigation bar (height 36px, mt 8px matching Slint y=8px)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .h(px(36.))
                    .px(px(8.))
                    .mt(px(4.))
                    // --- Back button (28x28, iconfont → ---
                    .child(
                        div()
                            .w(px(28.))
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, {
                                let this = this.clone();
                                move |_ev, _window, cx| {
                                    this.update(cx, |_panel, cx| {
                                        cx.emit(SettingsEvent::Back);
                                    });
                                }
                            })
                            .child(
                                div()
                                    .font_family("iconfont")
                                    .text_size(px(16.))
                                    .text_color(theme.text_2)
                                    .child("\u{e62b}"),
                            ),
                    )
                    // --- Title (14px, 700 weight, text_1) ---
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_1)
                            .child(I18nKey::SettingsTitle.text()),
                    ),
            )
            //  Tab bar (height 36px, mt 8px matching Slint spacing)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .h(px(36.))
                    .px(px(8.))
                    .mt(px(2.))
                    .border_b(px(1.))
                    .border_color(theme.divider)
                    .children(tab_names().iter().enumerate().map(|(i, name)| {
                        let is_active = i == active;
                        let tab_color = if is_active {
                            theme.accent
                        } else {
                            theme.text_2
                        };
                        let underline_bg = if is_active {
                            theme.accent
                        } else {
                            rgba(0x00000000)
                        };
                        let this = this.clone();

                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                this.update(cx, |panel, cx| {
                                    panel.set_active_tab(i);
                                    panel.close_app_list_popups();
                                    panel.latest_hotkeys_popup_open = false;
                                    cx.emit(SettingsEvent::TabChanged(i));
                                    cx.notify();
                                });
                            })
                            // --- Tab label ---
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(if is_active {
                                        FontWeight::BOLD
                                    } else {
                                        FontWeight::default()
                                    })
                                    .text_color(tab_color)
                                    .child(*name),
                            )
                            // --- Active underline indicator (2px) ---
                            .child(div().w_full().h(px(2.)).mt(px(4.)).bg(underline_bg))
                    })),
            )
            // --- Tab content (fills remaining space, scrollable) ---
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .rounded_b(px(12.))
                    .bg(theme.bg)
                    .pt(px(8.))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .w_full()
                            .overflow_hidden()
                            .child(
                                div()
                                    .id("settings-scroll-area")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .track_scroll(&self.scroll_handle)
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .flex_col()
                                            .px(px(8.))
                                            .opacity(tab_opacity)
                                            .mt(px(tab_offset))
                                            .when(active != 5, |el| el.pb(px(56.)))
                                            .child(match active {
                                                0 => self
                                                    .render_general_tab(window, cx)
                                                    .into_any_element(),
                                                1 => self
                                                    .render_clipboard_tab(window, cx)
                                                    .into_any_element(),
                                                2 => self
                                                    .render_hotkey_tab(window, cx)
                                                    .into_any_element(),
                                                3 => self
                                                    .render_data_tab(window, cx)
                                                    .into_any_element(),
                                                4 => self
                                                    .render_sync_tab(window, cx)
                                                    .into_any_element(),
                                                5 => self
                                                    .render_version_tab(window, cx)
                                                    .into_any_element(),
                                                _ => div().into_any_element(),
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(4.))
                                    .right(px(0.))
                                    .bottom(px(10.))
                                    .w(px(16.))
                                    .child(
                                        Scrollbar::vertical(&self.scroll_handle)
                                            .scrollbar_show(ScrollbarShow::Scrolling),
                                    ),
                            ),
                    )
                    // --- Reset data directory dialog (overlay) ---
                    .child(self.render_reset_data_dialog(window, cx).into_any_element()),
            )
            // --- Config sync dialogs (absolute overlay on root) ---
            .child(
                self.render_config_sync_dialogs(window, cx)
                    .into_any_element(),
            )
            // --- Delete backend dialog (absolute overlay on root) ---
            .child(
                self.render_delete_backend_dialog(window, cx)
                    .into_any_element(),
            )
    }
}

// --- Reusable control helpers ---

impl SettingsPanel {
    fn animation_running(started_at: Option<Instant>) -> bool {
        started_at.is_some_and(|started_at| {
            started_at.elapsed() <= TAB_ANIM_DURATION + Duration::from_millis(24)
        })
    }

    fn transition_f32(
        window: &mut Window,
        cx: &mut Context<Self>,
        key: (&'static str, u64),
        initial: f32,
        target: f32,
    ) -> f32 {
        let transition = window
            .use_keyed_transition(key, cx, TAB_ANIM_DURATION, move |_, _| initial)
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

    /// Render a settings row with an animated toggle switch on the right.
    fn setting_row_with_toggle(
        &mut self,
        label: &str,
        desc: &str,
        value: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_toggle: impl Fn(&mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = &self.theme;
        let surface = theme.surface;
        let divider = theme.divider;
        let accent = theme.accent;
        let text_1 = theme.text_1;
        let text_3 = theme.text_3;

        div()
            .h(px(66.))
            .rounded(px(10.))
            .bg(surface)
            .border(px(1.))
            .border_color(divider)
            .px(px(14.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.))
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .max_w_full()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .max_w_full()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(10.))
                            .text_color(text_3)
                            .child(desc.to_string()),
                    ),
            )
            .child(div().flex_shrink_0().child(render_toggle(
                value,
                label,
                ToggleColors {
                    accent,
                    track_off: divider,
                },
                &mut self.toggle_states,
                window,
                cx,
                on_toggle,
            )))
    }

    /// Render a toggle row with common boilerplate handled automatically.
    ///
    /// Handles entity cloning, dynamic description text, and `cx.notify()`.
    /// The `on_changed` closure receives references to the cloned state and
    /// settings-panel entities plus `&mut Window` and `&mut App`.
    #[allow(clippy::too_many_arguments)]
    fn render_toggle_row(
        &mut self,
        label: I18nKey,
        desc_on: I18nKey,
        desc_off: I18nKey,
        value: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_changed: impl Fn(&Entity<AppState>, &Entity<SettingsPanel>, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let this = cx.entity().clone();
        let desc = if value {
            desc_on.text()
        } else {
            desc_off.text()
        };
        self.setting_row_with_toggle(label.text(), desc, value, window, cx, move |window, app| {
            on_changed(&state, &this, window, app);
            this.update(app, |_panel, cx| cx.notify());
        })
    }

    /// Render a settings row with an option button group on the right.
    fn setting_row_with_options(
        &self,
        label: &str,
        desc: &str,
        options: &[(&'static str, &'static str)],
        active_key: &str,
        on_select: impl Fn(&'static str, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = &self.theme;
        let surface = theme.surface;
        let divider = theme.divider;
        let accent = theme.accent;
        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;
        let on_select = std::rc::Rc::new(on_select);

        div()
            .h(px(66.))
            .rounded(px(10.))
            .bg(surface)
            .border(px(1.))
            .border_color(divider)
            .px(px(14.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            // --- Left: label + description ---
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.))
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .max_w_full()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .max_w_full()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(10.))
                            .text_color(text_3)
                            .child(desc.to_string()),
                    ),
            )
            // --- Right: option buttons ---
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .flex_row()
                    .gap(px(4.))
                    .children(options.iter().map(|(key, display_label)| {
                        let selected = *key == active_key;
                        let btn_bg = if selected { accent } else { rgba(0x00000000) };
                        let btn_text = if selected { rgb(0xffffff) } else { text_2 };
                        let btn_weight = if selected {
                            FontWeight::BOLD
                        } else {
                            FontWeight::default()
                        };
                        let key = *key;
                        let on_select = on_select.clone();

                        div()
                            .h(px(26.))
                            .rounded(px(7.))
                            .px(px(8.))
                            .bg(btn_bg)
                            .when(!selected, |d| d.border(px(1.)).border_color(divider))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                on_select(key, _window, cx);
                            })
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .font_weight(btn_weight)
                                    .text_color(btn_text)
                                    .child(*display_label),
                            )
                    })),
            )
    }
    /// Clear pending hotkey confirm dialog.
    pub fn clear_hotkey_confirm(&mut self, cx: &mut Context<Self>) {
        self.hotkey_confirm = None;
        cx.notify();
    }

    /// Clear pending paste shortcut recording state.
    pub fn clear_paste_shortcut_state(&mut self, cx: &mut Context<Self>) {
        self.recording_paste_shortcut = None;
        self.pending_paste_shortcut = None;
        cx.notify();
    }
}
