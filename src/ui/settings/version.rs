//! Version settings tab — current version, update check, download progress, release notes.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::scroll::ScrollableElement;
use gpui_component::text::{TextView, TextViewStyle};

use crate::core::i18n_keys::I18nKey;
use crate::services::update::UpdatePhase;

use super::SettingsPanel;

impl SettingsPanel {
    pub fn render_version_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = &self.theme;
        let wm = self.window_manager.clone();
        let this = cx.entity().clone();

        let app = self.state.read(cx);
        let phase = app.update_phase.clone();
        let update_available = app.update_available.clone();
        let auto_check = app.settings.auto_check_updates;
        let current_version = env!("CARGO_PKG_VERSION");
        let surface = theme.surface;
        let divider = theme.divider;
        let accent = theme.accent;
        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;

        let (status_text, btn_label, btn_disabled, mut on_click) =
            button_state(&phase, &update_available, &wm);
        let show_release_notes = update_available.is_some()
            && phase != UpdatePhase::Idle
            && phase != UpdatePhase::Checking
            && phase != UpdatePhase::UpToDate;

        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .pt(px(8.))
            // ── Version info card ──
            .child(
                div()
                    .relative()
                    .rounded(px(10.))
                    .bg(surface)
                    .border(px(1.))
                    .border_color(divider)
                    .px(px(10.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    // Version number
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(format!("Clippi v{}", current_version)),
                    )
                    // Status text (may wrap, below version)
                    .child(
                        div()
                            .w_full()
                            .text_size(px(10.))
                            .text_color(if matches!(phase, UpdatePhase::Error(_)) {
                                theme.danger
                            } else {
                                accent
                            })
                            .child(status_text),
                    )
                    // Action row: auto-check checkbox + action button
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            // ── Left: compact auto-check checkbox ──
                            .child({
                                let this = this.clone();
                                let check_icon = if auto_check { "\u{e61f}" } else { "\u{e831}" };
                                let check_color = if auto_check { accent } else { text_3 };
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(4.))
                                    .cursor(CursorStyle::PointingHand)
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                        this.update(cx, |panel, cx| {
                                            panel.state.update(cx, |s, _cx| {
                                                s.settings.auto_check_updates =
                                                    !s.settings.auto_check_updates;
                                                s.settings.save();
                                            });
                                            cx.notify();
                                        });
                                    })
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_family("iconfont")
                                            .text_color(check_color)
                                            .child(check_icon),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(text_3)
                                            .child(I18nKey::SettingAutoCheckUpdate.text()),
                                    )
                            })
                            // ── Right: action button ──
                            .child({
                                let mut btn = div()
                                    .h(px(28.))
                                    .rounded(px(7.))
                                    .px(px(12.))
                                    .bg(if btn_disabled { divider } else { accent })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(if btn_disabled {
                                                text_3
                                            } else {
                                                rgb(0xffffff)
                                            })
                                            .child(btn_label),
                                    );
                                if let (false, Some(handler)) = (btn_disabled, on_click.take()) {
                                    btn = btn
                                        .cursor(CursorStyle::PointingHand)
                                        .on_mouse_down(MouseButton::Left, handler);
                                }
                                btn
                            }),
                    )
                    // Progress bar (only during download)
                    // GitHub link icon (absolute top-right, doesn't affect card height)
                    .child(
                        div()
                            .absolute()
                            .top(px(4.))
                            .right(px(12.))
                            .group("gh-icon")
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, |_ev, _window, _cx| {
                                std::thread::spawn(|| {
                                    crate::services::update::open_releases_page(
                                        "https://github.com/Ruszero01/clippi/releases",
                                    );
                                });
                            })
                            // Gray icon (default)
                            .child(
                                div()
                                    .text_size(px(26.))
                                    .font_family("iconfont")
                                    .text_color(text_3)
                                    .child("\u{ea0a}"),
                            )
                            // Green icon (shown on group hover)
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .text_size(px(26.))
                                    .font_family("iconfont")
                                    .text_color(accent)
                                    .opacity(0.0)
                                    .group_hover("gh-icon", |s| s.opacity(1.0))
                                    .child("\u{ea0a}"),
                            ),
                    )
                    .when(matches!(phase, UpdatePhase::Downloading { .. }), |el| {
                        el.child(render_progress_bar(
                            match phase {
                                UpdatePhase::Downloading { progress } => progress,
                                _ => 0,
                            },
                            accent,
                            divider,
                        ))
                    }),
            )
            // ── Release notes (shown when update found, fills remaining space) ──
            .when(show_release_notes, |el| {
                let notes = update_available
                    .as_ref()
                    .map(|i| i.release_notes.clone())
                    .unwrap_or_default();
                if notes.is_empty() {
                    return el;
                }
                el.child(
                    div()
                        .rounded(px(10.))
                        .bg(surface)
                        .border(px(1.))
                        .border_color(divider)
                        .flex()
                        .flex_col()
                        .child(
                            // Title inside the box
                            div()
                                .px(px(10.))
                                .pt(px(8.))
                                .pb(px(4.))
                                .border_b(px(1.))
                                .border_color(divider)
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(text_1)
                                        .child(I18nKey::VersionReleaseNotes.text()),
                                ),
                        )
                        .child({
                            let style = TextViewStyle::default()
                                .paragraph_gap(rems(0.2))
                                .heading_font_size(|level, base| {
                                    // Scale headings down to fit the compact settings UI.
                                    let sizes =
                                        [px(12.), px(11.5), px(11.), px(10.5), px(10.), px(10.)];
                                    sizes
                                        .get(level.saturating_sub(1) as usize)
                                        .copied()
                                        .unwrap_or(base)
                                });
                            div()
                                .px(px(10.))
                                .py(px(6.))
                                .max_h(px(200.))
                                .overflow_y_scrollbar()
                                .text_size(px(11.))
                                .text_color(text_2)
                                .child(
                                    TextView::markdown("version-release-notes", notes, window, cx)
                                        .style(style)
                                        .selectable(false),
                                )
                        }),
                )
            })
    }
}

type ButtonClickHandler = Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;
type ButtonState = (String, String, bool, Option<ButtonClickHandler>);

/// Determine the unified button state from the current update phase.
fn button_state(
    phase: &UpdatePhase,
    update_available: &Option<crate::services::update::UpdateInfo>,
    wm: &gpui::Entity<crate::ui::window_manager::WindowManager>,
) -> ButtonState {
    match phase {
        UpdatePhase::Idle | UpdatePhase::UpToDate => (
            format!("✓ {}", I18nKey::VersionUpToDate.text()),
            I18nKey::VersionCheckNow.text().to_string(),
            false,
            {
                let wm = wm.clone();
                Some(Box::new(move |_ev, _window, cx| {
                    wm.update(cx, |wm, cx| wm.start_update_check(cx));
                }))
            },
        ),
        UpdatePhase::Checking => (
            I18nKey::VersionChecking.text().to_string(),
            I18nKey::VersionChecking.text().to_string(),
            true,
            None,
        ),
        UpdatePhase::UpdateAvailable => {
            let latest = update_available
                .as_ref()
                .map(|i| i.latest_version.clone())
                .unwrap_or_default();
            (
                format!("● {} v{}", I18nKey::VersionFound.text(), latest),
                I18nKey::VersionDownload.text().to_string(),
                false,
                {
                    let wm = wm.clone();
                    Some(Box::new(move |_ev, _window, cx| {
                        wm.update(cx, |wm, cx| wm.start_update_download(cx));
                    }))
                },
            )
        }
        UpdatePhase::Downloading { progress } => (
            I18nKey::VersionDownloading
                .text()
                .replace("{0}", &progress.to_string()),
            format!("{}%", progress),
            true,
            None,
        ),
        UpdatePhase::Verifying => (
            I18nKey::VersionVerifying.text().to_string(),
            I18nKey::VersionVerifying.text().to_string(),
            true,
            None,
        ),
        UpdatePhase::Installing => (
            I18nKey::VersionInstalling.text().to_string(),
            I18nKey::VersionInstalling.text().to_string(),
            true,
            None,
        ),
        UpdatePhase::ReadyToRestart => (
            I18nKey::VersionReady.text().to_string(),
            I18nKey::BtnRestartNow.text().to_string(),
            false,
            {
                let wm = wm.clone();
                Some(Box::new(move |_ev, _window, cx| {
                    wm.update(cx, |wm, cx| wm.do_update_restart(cx));
                }))
            },
        ),
        UpdatePhase::Error(msg) => (
            msg.clone(),
            I18nKey::VersionCheckNow.text().to_string(),
            false,
            {
                let wm = wm.clone();
                Some(Box::new(move |_ev, _window, cx| {
                    wm.update(cx, |wm, cx| wm.start_update_check(cx));
                }))
            },
        ),
    }
}

/// Simple progress bar (colored fill inside a track).
fn render_progress_bar(progress: u8, fill_color: Rgba, track_color: Rgba) -> impl IntoElement {
    let pct = progress.min(100);
    div()
        .w_full()
        .h(px(4.))
        .rounded(px(2.))
        .bg(track_color)
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(relative(pct as f32 / 100.0))
                .rounded(px(2.))
                .bg(fill_color),
        )
}
