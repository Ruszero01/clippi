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

use super::clipboard_list::ClipboardListView;
use super::search_bar::SearchBar;
use super::settings::SettingsPanel;
use super::sidebar::Sidebar;
use super::tag_filter::TagFilterPanel;
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
        let list_view = cx.new(|cx| ClipboardListView::new(items, state.clone(), cx));
        list_view.update(cx, |list, _cx| list.focus(window));
        let titlebar = cx.new(|_cx| Titlebar::new(state.clone(), list_view.clone()));
        let search_bar = cx.new(|cx| SearchBar::new(state.clone(), list_view.clone(), window, cx));
        let settings_panel = cx.new(|cx| SettingsPanel::new(cx));
        let sidebar = cx.new(|_cx| Sidebar::new(state.clone(), list_view.clone()));
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
                    // TODO: Switch to settings view when settings panel
                    // is fully migrated to GPUI.
                    // this.current_view = "settings".into();
                    // cx.notify();
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
        ];
        let theme = ClippiTheme::dark();

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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = self.sidebar.clone();
        let titlebar = self.titlebar.clone();
        let list_view = self.list_view.clone();
        let search_bar = self.search_bar.clone();
        let settings_panel = self.settings_panel.clone();
        let tag_filter_panel = self.tag_filter_panel.clone();
        let tag_panel_open = self.search_bar.read(cx).tag_panel_open();
        let is_clipboard = self.current_view == "clipboard";
        let theme = &self.theme;

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
                    .border_color(theme.divider)
                    .flex()
                    .flex_col()
                    .occlude()
                    .child(titlebar)
                    .when(is_clipboard, |panel| {
                        panel.child(search_bar.clone()).child(list_view.clone())
                    })
                    .when(!is_clipboard, |panel| panel.child(settings_panel)),
            )
            .when(tag_panel_open && is_clipboard, |root| {
                let search_for_backdrop = search_bar.clone();
                root.child(
                    // Backdrop
                    div().absolute().size_full().on_mouse_down(
                        MouseButton::Left,
                        move |_ev, _window, cx| {
                            search_for_backdrop.update(cx, |bar, cx| bar.close_tag_panel(cx));
                        },
                    ),
                )
                .child(
                    // Panel — occluded to prevent click-through to backdrop
                    div()
                        .absolute()
                        .right(px(8.))
                        .top(px(106.))
                        .occlude()
                        .child(tag_filter_panel),
                )
            })
    }
}
