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

use crate::ui::font::fs;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::InputState;
use gpui_component::scroll::{Scrollbar, ScrollbarAxis, ScrollbarShow};
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
use crate::ui::components::slider::{
    SliderDetent, SliderDragState, SteppedSlider, SteppedSliderColors,
};
use crate::ui::components::toggle::{render_toggle, ToggleColors, ToggleTransitionState};use crate::ui::theme::ClippiTheme;
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
    /// Font scale or family changed — RootView should recompute cached card
    /// heights (they bake in the live scale) and re-render the whole tree.
    FontChanged,
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
    /// Upload confirmation dialog generation (incremented on each new dialog).
    pub config_sync_upload_confirm_gen: u64,
    /// Apply confirmation dialog generation (incremented on each new dialog).
    pub config_sync_apply_confirm_gen: u64,
    /// When the apply confirmation was shown.
    pub config_sync_apply_confirm_started: Option<Instant>,
    /// Whether the config-sync backend selector dropdown is open.
    pub config_sync_menu_open: bool,

    // ── Font picker ──
    /// Whether the custom font-family picker overlay is open.
    pub font_picker_open: bool,
    /// Lazily-enumerated installed font families (populated the first time the
    /// picker opens so we never re-scan the system font list on every render).
    available_fonts: Option<Rc<Vec<String>>>,
    /// Caller-owned drag state per slider id, so each slider's press/drag
    /// survives the re-renders every detent commit triggers.
    slider_drags: HashMap<&'static str, SliderDragState>,
    /// Frames left to align the font list with the current font after opening.
    /// The alignment needs the list's measured row bounds, which are only
    /// available once it has been laid out, so it is retried for a few frames
    /// and then stops (a settled list must not fight the user's scrolling).
    font_picker_scroll_frames: u8,
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
            config_sync_upload_confirm_gen: 0,
            config_sync_apply_confirm_gen: 0,
            config_sync_apply_confirm_started: None,
            config_sync_menu_open: false,
            font_picker_open: false,
            available_fonts: None,
            slider_drags: HashMap::new(),
            font_picker_scroll_frames: 0,
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
        // Bump the animation generation when the apply dialog appears so it
        // plays the same enter animation as the other confirm dialogs.
        if show_apply {
            if self.config_sync_apply_confirm_started.is_none() {
                self.config_sync_apply_confirm_gen =
                    self.config_sync_apply_confirm_gen.wrapping_add(1);
                self.config_sync_apply_confirm_started = Some(Instant::now());
            }
        } else {
            self.config_sync_apply_confirm_started = None;
        }

        div()
            .when(show_upload || show_apply, |root| {
                // Positioning context only — ConfirmDialog provides its own
                // backdrop, so no second dimming layer is drawn here.
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
                        .render_animated(window, cx, self.config_sync_upload_confirm_gen),
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
            // Positioning context only — ConfirmDialog provides its own
            // backdrop, so no second dimming layer is drawn here.
            root.absolute().size_full().top_0().left_0().child(
                ConfirmDialog::delete_backend()
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
                                    .text_size(fs(16.))
                                    .text_color(theme.text_2)
                                    .child("\u{e62b}"),
                            ),
                    )
                    // --- Title (14px, 700 weight, text_1) ---
                    .child(
                        div()
                            .text_size(fs(14.))
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
                                    panel.font_picker_open = false;
                                    cx.emit(SettingsEvent::TabChanged(i));
                                    cx.notify();
                                });
                            })
                            // --- Tab label ---
                            .child(
                                div()
                                    .text_size(fs(12.))
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
            // --- Font family picker (absolute overlay on root) ---
            .child(self.render_font_picker(window, cx).into_any_element())
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

    /// A labelled group of settings rows.
    ///
    /// The section title sits above a single card that holds every row of the
    /// group, with hairline dividers between rows — so related settings read as
    /// one block instead of a wall of separate cards. Rows are rendered without
    /// their own card chrome (see the `setting_row_*` helpers).
    ///
    /// A group holding a single row is self-evident, so it is rendered without a
    /// title (the row's own label already names it).
    pub(crate) fn settings_group(&self, title: &str, rows: Vec<AnyElement>) -> impl IntoElement {
        self.settings_group_with(title, rows, false)
    }

    /// Like [`Self::settings_group`], but the group stretches to fill the height
    /// left over by its parent. Used by the version tab, where the release-notes
    /// area is the only part that scrolls — so the page itself never scrolls and
    /// there is just one scrollbar.
    pub(crate) fn settings_group_fill(&self, title: &str, rows: Vec<AnyElement>) -> impl IntoElement {
        self.settings_group_with(title, rows, true)
    }

    fn settings_group_with(
        &self,
        title: &str,
        rows: Vec<AnyElement>,
        stretch: bool,
    ) -> impl IntoElement {
        let theme = &self.theme;
        let divider = theme.divider;
        let text_3 = theme.text_3;
        let show_title = rows.len() > 1;

        let mut card = div()
            .rounded(px(10.))
            .bg(theme.surface)
            .border(px(1.))
            .border_color(divider)
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(stretch, |card| card.flex_1().min_h(px(0.)));
        for (i, row) in rows.into_iter().enumerate() {
            if i > 0 {
                // Inset hairline: separates rows without boxing each of them.
                card = card.child(div().h(px(1.)).mx(px(14.)).bg(divider));
            }
            card = card.child(row);
        }

        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .when(stretch, |group| group.flex_1().min_h(px(0.)))
            .when(show_title, |group| {
                group.child(
                    div()
                        .px(px(4.))
                        .text_size(fs(10.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text_3)
                        .child(title.to_string()),
                )
            })
            .child(card)
    }

    /// The bare shell every settings row shares: standard row padding and
    /// minimum height, no card chrome (the enclosing group provides it). Use it
    /// directly when a row's control is bespoke, so it still lines up with the
    /// rows built from the `setting_row_*` helpers.
    pub(crate) fn row_shell(&self) -> Div {
        div()
            .min_h(px(66.))
            .px(px(14.))
            .py(px(12.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(10.))
    }

    /// The label + description column of a settings row.
    ///
    /// The label stays on one line (ellipsised); the description wraps and the
    /// row grows, so text never slides under the control on the right.
    pub(crate) fn row_text(&self, label: &str, desc: &str) -> Div {
        let theme = &self.theme;
        let text_1 = theme.text_1;
        let text_3 = theme.text_3;
        div()
            .flex()
            .flex_1()
            .min_w(px(0.))
            .flex_col()
            .gap(px(3.))
            .child(
                div()
                    .max_w_full()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(fs(12.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(text_1)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .w_full()
                    .text_size(fs(10.))
                    .line_height(fs(14.))
                    .text_color(text_3)
                    .child(desc.to_string()),
            )
    }

    /// A tappable row that opens something else (a list, a picker, a dialog):
    /// label + description on the left, a chevron on the right. Used by the
    /// blacklist / shortcut / latest-hotkey entries so they share one shape.
    pub(crate) fn setting_row_opener(
        &self,
        label: &str,
        desc: &str,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = &self.theme;
        let text_2 = theme.text_2;
        let hover_bg = theme.titlebar_bg;
        let on_click = Rc::new(on_click);
        self.row_shell()
            .cursor(CursorStyle::PointingHand)
            .hover(move |style| style.bg(hover_bg))
            .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                cx.stop_propagation();
                on_click(window, cx);
            })
            .child(self.row_text(label, desc))
            .child(
                div()
                    .flex_shrink_0()
                    .font_family("iconfont")
                    .text_size(fs(14.))
                    .text_color(text_2)
                    .child("\u{e602}"),
            )
    }

    /// Render a settings row with an animated toggle switch on the right.
    ///
    /// Single-control row: kept on one line, but the description wraps and the
    /// card grows instead of the text ever sliding under the toggle.
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
        let divider = theme.divider;
        let accent = theme.accent;

        self.row_shell()
            .child(self.row_text(label, desc))
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

    /// Render a settings card for option groups (≥2 buttons).
    ///
    /// Two-row layout mirroring the copy-sound / sync-backend cards: the
    /// header holds the label and a description that may wrap to any number of
    /// lines, a divider separates it from the footer, and the option buttons
    /// fill the footer row with equal widths. Because the buttons never share a
    /// line with the text, larger font sizes can't overlap the description.
    fn setting_row_with_options(
        &self,
        label: &str,
        desc: &str,
        options: &[(&'static str, &'static str)],
        active_key: &str,
        on_select: impl Fn(&'static str, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = &self.theme;
        let divider = theme.divider;
        let accent = theme.accent;
        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;
        let on_select = std::rc::Rc::new(on_select);

        div()
            .flex()
            .flex_col()
            // --- Header: label + wrapping description ---
            .child(
                div()
                    .px(px(14.))
                    .pt(px(12.))
                    .pb(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .max_w_full()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(fs(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_size(fs(10.))
                            .line_height(fs(14.))
                            .text_color(text_3)
                            .child(desc.to_string()),
                    ),
            )
            // --- Divider ---
            .child(div().h(px(1.)).bg(divider))
            // --- Footer: equal-width option buttons on their own row ---
            .child(
                div()
                    .px(px(10.))
                    .py(px(9.))
                    .flex()
                    .flex_row()
                    .gap(px(6.))
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
                            .flex_1()
                            .min_w(px(0.))
                            .h(px(28.))
                            .rounded(px(7.))
                            .bg(btn_bg)
                            .when(!selected, |d| d.border(px(1.)).border_color(divider))
                            .flex()
                            .items_center()
                            .justify_center()
                            .overflow_hidden()
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                on_select(key, _window, cx);
                            })
                            .child(
                                div()
                                    .max_w_full()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(fs(11.))
                                    .font_weight(btn_weight)
                                    .text_color(btn_text)
                                    .child(*display_label),
                            )
                    })),
            )
    }

    /// The layered palette every slider in the settings shares: track < fill <
    /// ticks < knob, derived from the theme accent so it adapts to light/dark.
    pub(crate) fn slider_colors(&self) -> SteppedSliderColors {
        let theme = &self.theme;
        let track_off = if theme.bg == rgb(0x191a1b) {
            rgb(0x3a3b3c)
        } else {
            rgb(0xd0d2de)
        };
        SteppedSliderColors {
            accent: theme.accent,
            fill: mix(track_off, theme.accent, 0.55),
            track_off,
            tick_passed: theme.accent,
            tick_off: mix(track_off, theme.text_2, 0.35),
            knob_ring: mix(theme.accent, rgb(0xffffff), 0.45),
            knob_core: mix(theme.accent, rgb(0x000000), 0.22),
            badge_bg: theme.titlebar_bg,
            text: theme.text_2,
        }
    }

    /// Caller-owned drag state for the slider with `id`, created on first use.
    pub(crate) fn slider_drag(&mut self, id: &'static str, initial: usize) -> SliderDragState {
        self.slider_drags
            .entry(id)
            .or_insert_with(|| SliderDragState::new(initial))
            .clone()
    }

    /// Two-row settings card whose footer hosts a stepped slider (used by the
    /// font-size entry). Mirrors `setting_row_with_options` layout: wrapping
    /// header text, a divider, then the slider on its own full-width row.
    #[allow(clippy::too_many_arguments)]
    fn setting_row_with_slider(
        &mut self,
        label: &str,
        desc: &str,
        detents: Vec<SliderDetent>,
        active: usize,
        slider_id: &'static str,
        on_change: impl Fn(usize, &mut Window, &mut App) + 'static,
        on_preview: impl Fn(&mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let drag = self.slider_drag(slider_id, active);
        let theme = &self.theme;
        let divider = theme.divider;
        let text_1 = theme.text_1;
        let text_3 = theme.text_3;
        let colors = self.slider_colors();

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(14.))
                    .pt(px(12.))
                    .pb(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .max_w_full()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(fs(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_size(fs(10.))
                            .line_height(fs(14.))
                            .text_color(text_3)
                            .child(desc.to_string()),
                    ),
            )
            .child(div().h(px(1.)).bg(divider))
            .child(
                div()
                    .px(px(16.))
                    .pt(px(10.))
                    .pb(px(8.))
                    .child(
                        SteppedSlider::new(slider_id, detents)
                            .value(active)
                            .label_inset(18.)
                            .drag_state(drag)
                            .colors(colors)
                            .on_change(on_change)
                            .on_preview(on_preview),
                    ),
            )
    }

    /// Render a settings row whose right side is a single value button that
    /// opens a picker (used by the custom-font entry).
    fn setting_row_with_value(
        &self,
        label: &str,
        desc: &str,
        value: String,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = &self.theme;
        let divider = theme.divider;
        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;
        let on_click = Rc::new(on_click);

        div()
            .min_h(px(66.))
            .px(px(14.))
            .py(px(12.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(10.))
            // --- Left: label + wrapping description ---
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.))
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .max_w_full()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(fs(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_size(fs(10.))
                            .line_height(fs(14.))
                            .text_color(text_3)
                            .child(desc.to_string()),
                    ),
            )
            // --- Right: current value + caret (never compresses, never overlaps) ---
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .h(px(26.))
                    .px(px(10.))
                    .rounded(px(7.))
                    .border(px(1.))
                    .border_color(divider)
                    .cursor(CursorStyle::PointingHand)
                    .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                        cx.stop_propagation();
                        on_click(window, cx);
                    })
                    .child(
                        div()
                            .max_w(px(150.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(fs(11.))
                            .text_color(text_2)
                            .child(value),
                    )
                    .child(div().text_size(fs(12.)).text_color(text_3).child("›")),
            )
    }

    /// Open the custom-font picker, enumerating installed families on first use
    /// so the system font list is scanned at most once per panel lifetime.
    pub fn open_font_picker(&mut self, cx: &mut Context<Self>) {
        if self.available_fonts.is_none() {
            self.available_fonts = Some(Rc::new(crate::ui::font::available_families(cx)));
        }
        self.close_app_list_popups();
        self.latest_hotkeys_popup_open = false;
        self.config_sync_menu_open = false;
        self.font_picker_open = true;
        self.font_picker_scroll_frames = 4;
        cx.notify();
    }

    /// Close the custom-font picker (state only — the caller must notify).
    pub fn close_font_picker(&mut self, cx: &mut Context<Self>) {
        self.font_picker_open = false;
        cx.notify();
    }

    /// Apply a chosen font family and notify the window to re-render.
    fn apply_font_family(&mut self, family: String, cx: &mut Context<Self>) {
        let state = self.state.clone();
        state.update(cx, |s, _cx| {
            s.settings.font_family = family.clone();
            s.settings.save();
        });
        crate::ui::font::set_family(&family);
        crate::ui::font::apply_to_global_theme(cx);
        self.font_picker_open = false;
        cx.emit(SettingsEvent::FontChanged);
        cx.notify();
    }

    /// Absolute overlay covering the settings panel: a dim, inset backdrop with
    /// a centered, size-capped card holding a scrollable (with scrollbar) list
    /// of font families, each previewed in its own typeface.
    fn render_font_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if !self.font_picker_open {
            return div().into_any_element();
        }
        let theme = &self.theme;
        let surface = theme.surface;
        let divider = theme.divider;
        let accent = theme.accent;
        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;
        let hover_bg = if theme.bg == rgb(0x191a1b) {
            rgba(0xffffff10)
        } else {
            rgba(0x0000000a)
        };
        let fonts = self.available_fonts.clone().unwrap_or_default();
        let current = crate::ui::font::family();
        let system_label = I18nKey::FontFamilySystem.text();
        let sample = I18nKey::FontFamilySample.text();
        let this = cx.entity().clone();

        // Persistent scroll handle for the family list. On open we align the
        // row matching the active font with the top of the list, so the current
        // choice is immediately visible instead of starting at the top of a
        // long list.
        let scroll_handle = window
            .use_keyed_state(
                ElementId::Name("font-picker-scroll-handle".into()),
                cx,
                |_, _| ScrollHandle::default(),
            )
            .read(cx)
            .clone();
        if self.font_picker_scroll_frames > 0 {
            self.font_picker_scroll_frames -= 1;
            let current_index = if current.is_empty() {
                0
            } else {
                fonts
                    .iter()
                    .position(|f| *f == current)
                    .map_or(0, |i| i + 1)
            };
            // Compute the offset from the *measured* row bounds instead of
            // asking the handle to scroll and hoping the request lands: the row
            // position only exists after layout, and an early offset is what
            // left the target off screen.
            if let Some(row) = scroll_handle.bounds_for_item(current_index) {
                let viewport_top = f32::from(scroll_handle.bounds().top());
                let row_top = f32::from(row.top());
                let max_scroll = f32::from(scroll_handle.max_offset().height);
                let y = px((viewport_top - row_top).clamp(-max_scroll, 0.0));
                scroll_handle.set_offset(point(px(0.), y));
            }
            // Repaint until the alignment has been applied (the row bounds may
            // not exist on the very first frame).
            let this = this.clone();
            window.defer(cx, move |_window, cx| {
                this.update(cx, |_panel, cx| cx.notify());
            });
        }

        let row_bg_transparent = rgba(0x00000000);
        let row_text_selected = rgb(0xffffff);
        let mk_entry = |family: String,
                        display: String,
                        preview_family: SharedString|
         -> AnyElement {
            let selected = family == current;
            let id = if family.is_empty() {
                "font-family-system".to_string()
            } else {
                format!("font-family-{}", family)
            };
            div()
                .id(SharedString::from(id))
                .h(px(34.))
                .flex_shrink_0()
                .px(px(10.))
                .mx(px(4.))
                .rounded(px(6.))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .bg(if selected { accent } else { row_bg_transparent })
                .cursor(CursorStyle::PointingHand)
                .when(!selected, |d| d.hover(move |s| s.bg(hover_bg)))
                .on_mouse_down(MouseButton::Left, {
                    let this = this.clone();
                    move |_ev, _window, cx| {
                        let family = family.clone();
                        this.update(cx, |panel, cx| panel.apply_font_family(family, cx));
                    }
                })
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .font_family(preview_family.clone())
                        .text_size(fs(13.))
                        .text_color(if selected {
                            row_text_selected
                        } else {
                            text_2
                        })
                        .child(display),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .ml(px(8.))
                        .font_family(preview_family)
                        .text_size(fs(11.))
                        .text_color(if selected {
                            row_text_selected
                        } else {
                            text_3
                        })
                        .child(sample),
                )
                .into_any_element()
        };

        let entries: Vec<AnyElement> = std::iter::once(mk_entry(
            String::new(),
            system_label.to_string(),
            crate::ui::font::SYSTEM_UI_FONT.into(),
        ))
        .chain(
            fonts
                .iter()
                .map(|name| mk_entry(name.clone(), name.clone(), SharedString::from(name.clone()))),
        )
        .collect();

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .p(px(16.))
            .bg(rgba(0x00000070))
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .on_mouse_down(MouseButton::Left, {
                let this = this.clone();
                move |_ev, _window, cx| {
                    this.update(cx, |panel, cx| panel.close_font_picker(cx));
                }
            })
            .child(
                div()
                    .w_full()
                    .max_w(px(320.))
                    .h_full()
                    .max_h(px(460.))
                    .rounded(px(10.))
                    .bg(surface)
                    .border(px(1.))
                    .border_color(divider)
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .cursor(CursorStyle::Arrow)
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
                    // --- Header ---
                    .child(
                        div()
                            .h(px(44.))
                            .flex_shrink_0()
                            .px(px(14.))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(fs(13.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(text_1)
                                    .child(I18nKey::FontFamilyPicker.text()),
                            )
                            .child(
                                div()
                                    .w(px(24.))
                                    .h(px(24.))
                                    .rounded(px(6.))
                                    .font_family("iconfont")
                                    .text_size(fs(12.))
                                    .text_color(text_2)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .on_mouse_down(MouseButton::Left, {
                                        let this = this.clone();
                                        move |_ev, _window, cx| {
                                            this.update(cx, |panel, cx| {
                                                panel.close_font_picker(cx)
                                            });
                                        }
                                    })
                                    .child("\u{e7b7}"),
                            ),
                    )
                    .child(div().h(px(1.)).flex_shrink_0().bg(divider))
                    // --- Scrollable family list + vertical scrollbar ---
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h(px(0.))
                            .child(
                                div()
                                    .id("font-picker-scroll")
                                    .size_full()
                                    .py(px(4.))
                                    .pr(px(6.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .overflow_y_scroll()
                                    .track_scroll(&scroll_handle)
                                    .children(entries),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .right_0()
                                    .bottom_0()
                                    .child(
                                        Scrollbar::new(&scroll_handle)
                                            .id("font-picker-scrollbar")
                                            .axis(ScrollbarAxis::Vertical),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
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

/// Linear blend between two colors; `t` is clamped to 0..=1. Used to derive
/// the layered slider palette from the theme accent.
fn mix(from: Rgba, to: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    Rgba {
        r: from.r + (to.r - from.r) * t,
        g: from.g + (to.g - from.g) * t,
        b: from.b + (to.b - from.b) * t,
        a: from.a + (to.a - from.a) * t,
    }
}
