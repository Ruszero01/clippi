//! --- Sync settings tab. ---

use std::time::Duration;

use gpui::prelude::*;
use gpui::*;
use gpui_component::scroll::ScrollableElement;
use gpui_component::tooltip::Tooltip;
use gpui_transitions::WindowUseTransition;

use crate::core::i18n_keys::I18nKey;
use crate::services::gpui_sync::format_last_sync;
use crate::state::sync::BackendStatus;
use crate::ui::components::toggle::{render_toggle, ToggleColors};

use super::{BackendCollapseState, SettingsPanel};

const COLLAPSE_DURATION: Duration = Duration::from_millis(300);

impl SettingsPanel {
    pub fn render_sync_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sync = self.state.read(cx).sync.clone();
        let wm = self.window_manager.clone();
        let backend_panel = self.backend_panel();
        let surface = self.theme.surface;
        let divider = self.theme.divider;
        let accent = self.theme.accent;
        let accent_soft = self.theme.accent_soft;
        let text_1 = self.theme.text_1;
        let text_2 = self.theme.text_2;
        let text_3 = self.theme.text_3;
        let transfer_enabled = self.state.read(cx).settings.transfer_station_enabled;
        let transfer_retention = self.state.read(cx).settings.transfer_retention_days;
        let enabled_backend_count = sync
            .backends
            .iter()
            .filter(|backend| backend.config.enabled)
            .count();

        let backend_cards: Vec<AnyElement> = sync
            .backends
            .iter()
            .map(|backend| {
                self.render_backend_card(backend, window, cx)
                    .into_any_element()
            })
            .collect();

        div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            .child(
                div()
                    .rounded(px(10.))
                    .bg(surface)
                    .border(px(1.))
                    .border_color(divider)
                    .overflow_hidden()
                    .child(
                        div()
                            .h(px(38.))
                            .px(px(14.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(text_1)
                                    .child(I18nKey::SyncTabTitle.text()),
                            )
                            .child(div().flex_shrink_0().child(render_toggle(
                                sync.auto_enabled,
                                "sync-auto-enabled",
                                ToggleColors {
                                    accent,
                                    track_off: divider,
                                },
                                &mut self.toggle_states,
                                window,
                                cx,
                                {
                                    let wm = wm.clone();
                                    move |_window, cx| {
                                        wm.update(cx, |wm, cx| {
                                            wm.toggle_sync_auto_enabled(cx);
                                        });
                                    }
                                },
                            ))),
                    )
                    .when(sync.auto_enabled, |card| {
                        card.child(div().h(px(1.)).bg(divider)).child(
                            div()
                                .h(px(38.))
                                .px(px(14.))
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(text_1)
                                        .child(I18nKey::SyncFavoritesOnly.text()),
                                )
                                .child(div().flex_shrink_0().child(render_toggle(
                                    sync.favorites_only,
                                    "sync-favorites-only",
                                    ToggleColors {
                                        accent,
                                        track_off: divider,
                                    },
                                    &mut self.toggle_states,
                                    window,
                                    cx,
                                    {
                                        let wm = wm.clone();
                                        move |_window, cx| {
                                            wm.update(cx, |wm, cx| {
                                                wm.toggle_sync_favorites_only(cx);
                                            });
                                        }
                                    },
                                ))),
                        )
                    }),
            )
            .when(sync.auto_enabled, |container| {
                container.child(
                    div()
                        .rounded(px(10.))
                        .bg(surface)
                        .border(px(1.))
                        .border_color(divider)
                        .overflow_hidden()
                        .child(
                            div()
                                .h(px(38.))
                                .px(px(14.))
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(text_1)
                                        .child(I18nKey::SyncIncludeImages.text()),
                                )
                                .child(div().flex_shrink_0().child(render_toggle(
                                    sync.include_images,
                                    "sync-include-images",
                                    ToggleColors {
                                        accent,
                                        track_off: divider,
                                    },
                                    &mut self.toggle_states,
                                    window,
                                    cx,
                                    {
                                        let wm = wm.clone();
                                        move |_window, cx| {
                                            wm.update(cx, |wm, cx| {
                                                wm.toggle_sync_include_images(cx);
                                            });
                                        }
                                    },
                                ))),
                        )
                        .when(sync.include_images, |card| {
                            card.child(div().h(px(1.)).bg(divider)).child(
                                div()
                                    .h(px(38.))
                                    .px(px(14.))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(text_1)
                                            .child(I18nKey::SyncCompressImages.text()),
                                    )
                                    .child(div().flex_shrink_0().child(render_toggle(
                                        sync.compress_images,
                                        "sync-compress-images",
                                        ToggleColors {
                                            accent,
                                            track_off: divider,
                                        },
                                        &mut self.toggle_states,
                                        window,
                                        cx,
                                        {
                                            let wm = wm.clone();
                                            move |_window, cx| {
                                                wm.update(cx, |wm, cx| {
                                                    wm.toggle_sync_compress_images(cx);
                                                });
                                            }
                                        },
                                    ))),
                            )
                        }),
                )
            })
            .child(
                div()
                    .rounded(px(10.))
                    .bg(surface)
                    .border(px(1.))
                    .border_color(divider)
                    .overflow_hidden()
                    .child(
                        div()
                            .h(px(38.))
                            .px(px(14.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(text_1)
                                            .child(I18nKey::TransferStation.text()),
                                    )
                                    .when(enabled_backend_count == 0, |row| {
                                        row.child(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(text_2)
                                                .child(I18nKey::TransferNoBackend.text()),
                                        )
                                    }),
                            )
                            .child(render_toggle(
                                transfer_enabled,
                                "transfer-station-enabled",
                                ToggleColors {
                                    accent,
                                    track_off: divider,
                                },
                                &mut self.toggle_states,
                                window,
                                cx,
                                {
                                    let wm = wm.clone();
                                    move |_window, cx| {
                                        wm.update(cx, |wm, cx| {
                                            wm.toggle_transfer_station(cx);
                                        });
                                    }
                                },
                            )),
                    )
                    .when(transfer_enabled, |card| {
                        card.child(div().h(px(1.)).bg(divider)).child(
                            div()
                                .h(px(38.))
                                .px(px(14.))
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .w(px(76.))
                                        .text_size(px(11.))
                                        .text_color(text_2)
                                        .child(I18nKey::TransferRetention.text()),
                                )
                                .children([0_u32, 1, 3, 7, 30].into_iter().map(|days| {
                                    let selected = transfer_retention == days;
                                    let wm = wm.clone();
                                    let label = if days == 0 {
                                        I18nKey::TransferKeepForever.text().to_string()
                                    } else {
                                        I18nKey::TransferRetentionDays.fmt(&[&days.to_string()])
                                    };
                                    div()
                                        .flex_1()
                                        .h(px(22.))
                                        .rounded(px(6.))
                                        .bg(if selected { accent } else { rgba(0x00000000) })
                                        .text_size(px(10.))
                                        .text_color(if selected { rgb(0xffffff) } else { text_2 })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(move |style| {
                                            if selected {
                                                style.opacity(0.88)
                                            } else {
                                                style.bg(accent_soft)
                                            }
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            move |_event, _window, cx| {
                                                wm.update(cx, |wm, cx| {
                                                    wm.set_transfer_retention_days(days, cx);
                                                });
                                            },
                                        )
                                        .child(label)
                                })),
                        )
                        .child(
                            div()
                                .px(px(14.))
                                .pb(px(8.))
                                .text_size(px(9.))
                                .text_color(text_3)
                                .child(I18nKey::TransferProtocolUpgradeRequired.text()),
                        )
                    }),
            )
            .child({
                // ── Config sync card ──
                let sync = self.state.read(cx).sync.clone();
                let config_backends: Vec<BackendStatus> = sync
                    .backends
                    .iter()
                    .filter(|b| b.config.backend_type == "local_folder" || b.config.backend_type == "webdav")
                    .cloned()
                    .collect();

                if self.config_sync_backend_id.is_none() {
                    self.config_sync_backend_id = config_backends.first().map(|b| b.config.id.clone());
                }

                // Revalidate the selected backend every render: if it was
                // deleted, fall back to the first available one so the buttons
                // never silently operate on a stale ID.
                let selected_still_exists = self
                    .config_sync_backend_id
                    .as_ref()
                    .is_some_and(|id| config_backends.iter().any(|b| &b.config.id == id));
                if !selected_still_exists {
                    self.config_sync_backend_id =
                        config_backends.first().map(|b| b.config.id.clone());
                }

                let selected_id = self.config_sync_backend_id.clone();
                let no_backends = config_backends.is_empty();
                let busy = wm.read(cx).is_config_sync_busy();
                let buttons_disabled = no_backends || busy;

                let selected_name = config_backends
                    .iter()
                    .find(|b| Some(&b.config.id) == selected_id.as_ref())
                    .map(|b| b.config.name.clone())
                    .unwrap_or_default();

                let ids: Vec<String> = config_backends.iter().map(|b| b.config.id.clone()).collect();
                let names: Vec<String> = config_backends.iter().map(|b| b.config.name.clone()).collect();
                let menu_open = self.config_sync_menu_open && ids.len() > 1;

                div()
                    .relative()
                    .rounded(px(10.))
                    .bg(surface)
                    .border(px(1.))
                    .border_color(divider)
                    .child(
                        div()
                            .overflow_hidden()
                            .rounded_t(px(10.))
                            .px(px(14.))
                            .py(px(10.))
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(text_1)
                                            .child(I18nKey::ConfigSyncTitle.text()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(text_2)
                                    .child(I18nKey::ConfigSyncDesc.text()),
                            ),
                    )
                    .child(div().h(px(1.)).bg(divider))
                    .child({

                        div()
                            .px(px(14.))
                            .py(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(text_2)
                                    .child(I18nKey::ConfigSyncTargetBackend.text()),
                            )
                            .child({
                                let this = cx.entity().clone();
                                div()
                                    .flex_1()
                                    .h(px(22.))
                                    .px(px(8.))
                                    .rounded(px(4.))
                                    .border(px(1.))
                                    .border_color(if menu_open { accent } else { divider })
                                    .bg(surface)
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .text_size(px(10.))
                                    .text_color(if no_backends { text_3 } else { accent })
                                    .cursor(if ids.len() <= 1 { CursorStyle::Arrow } else { CursorStyle::PointingHand })
                                    .opacity(if no_backends { 0.45 } else { 1.0 })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(if no_backends {
                                                I18nKey::ConfigSyncNoBackend.text().to_string()
                                            } else {
                                                selected_name.clone()
                                            }),
                                    )
                                    .when(ids.len() > 1, |el| {
                                        el.child(
                                            div()
                                                .ml(px(2.))
                                                .flex_shrink_0()
                                                .font_family("iconfont")
                                                .text_size(px(8.))
                                                .text_color(text_3)
                                                .child("\u{e602}"),
                                        )
                                    })
                                    .on_mouse_down(MouseButton::Left, {
                                        let ids = ids.clone();
                                        let this = this.clone();
                                        move |_ev, _window, cx| {
                                            if ids.len() <= 1 {
                                                return;
                                            }
                                            this.update(cx, |panel, cx| {
                                                panel.config_sync_menu_open = !panel.config_sync_menu_open;
                                                cx.notify();
                                            });
                                        }
                                    })
                            })
                    })
                    .child(div().h(px(1.)).bg(divider))
                    .child(
                        div()
                            .px(px(14.))
                            .py(px(8.))
                            .flex()
                            .gap(px(6.))
                            .child({
                                let _wm = wm.clone();
                                let selected = selected_id.clone();
                                let backend_name = selected_name.clone();
                                let this = cx.entity().clone();
                                div()
                                    .flex_1()
                                    .h(px(28.))
                                    .rounded(px(6.))
                                    .bg(accent)
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xffffff))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(if buttons_disabled { CursorStyle::Arrow } else { CursorStyle::PointingHand })
                                    .opacity(if buttons_disabled { 0.45 } else { 1.0 })
                                    .child(if busy { I18nKey::ConfigSyncUploading.text() } else { I18nKey::ConfigSyncUpload.text() })
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                        if buttons_disabled {
                                            return;
                                        }
                                        if let Some(ref _id) = selected {
                                            this.update(cx, |panel, cx| {
                                                panel.config_sync_upload_confirm = Some(backend_name.clone());
                                                panel.config_sync_upload_confirm_gen = panel
                                                    .config_sync_upload_confirm_gen
                                                    .wrapping_add(1);
                                                cx.notify();
                                            });
                                        }
                                    })
                            })
                            .child({
                                let wm = wm.clone();
                                let selected = selected_id.clone();
                                let this = cx.entity().clone();
                                div()
                                    .flex_1()
                                    .h(px(28.))
                                    .rounded(px(6.))
                                    .bg(accent_soft)
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(accent)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(if buttons_disabled { CursorStyle::Arrow } else { CursorStyle::PointingHand })
                                    .opacity(if buttons_disabled { 0.45 } else { 1.0 })
                                    .child(if busy { I18nKey::ConfigSyncDownloading.text() } else { I18nKey::ConfigSyncApply.text() })
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                        if buttons_disabled {
                                            return;
                                        }
                                        if let Some(ref id) = selected {
                                            let backends = this.read(cx).state.read(cx).settings.sync_backends.clone();
                                            if let Some(cfg) = backends.iter().find(|b| &b.id == id) {
                                                if let Some(backend) = crate::services::backends::create_config_snapshot_backend(cfg) {
                                                    wm.update(cx, |wm, _cx| {
                                                        wm.start_config_download(backend);
                                                    });
                                                }
                                            }
                                            this.update(cx, |_panel, cx| {
                                                cx.notify();
                                            });
                                        }
                                    })
                            }),
                    )
                    .when(menu_open, |card| {
                        let ids = ids.clone();
                        let names = names.clone();
                        let selected = selected_id.clone();
                        let this = cx.entity().clone();
                        card.child(
                            div()
                                .absolute()
                                .top(px(92.))
                                .left(px(60.))
                                .right(px(14.))
                                .rounded(px(6.))
                                .border(px(1.))
                                .border_color(divider)
                                .bg(surface)
                                .shadow_lg()
                                .p(px(4.))
                                .occlude()
                                .children(ids.iter().enumerate().map(|(i, id)| {
                                    let name = names[i].clone();
                                    let id = id.clone();
                                    let active = Some(&id) == selected.as_ref();
                                    let this = this.clone();
                                    div()
                                        .h(px(24.))
                                        .rounded(px(4.))
                                        .px(px(8.))
                                        .flex()
                                        .items_center()
                                        .text_size(px(10.))
                                        .text_color(if active { accent } else { text_1 })
                                        .bg(if active { accent_soft } else { rgba(0x00000000) })
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(move |style| {
                                            if !active {
                                                style.bg(accent_soft)
                                            } else {
                                                style
                                            }
                                        })
                                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                            this.update(cx, |panel, cx| {
                                                panel.config_sync_backend_id = Some(id.clone());
                                                panel.config_sync_menu_open = false;
                                                cx.notify();
                                            });
                                        })
                                        .child(name)
                                })),
                        )
                    })
            })
            .child(
                div()
                    .rounded(px(10.))
                    .bg(surface)
                    .border(px(1.))
                    .border_color(divider)
                    .overflow_hidden()
                    .child(
                        div()
                            .h(px(40.))
                            .rounded(px(10.))
                            .bg(self.theme.titlebar_bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(6.))
                            .when(sync.auto_enabled, |button| {
                                button
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(move |style| style.bg(accent_soft))
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                        backend_panel.update(cx, |panel, cx| {
                                            panel.open_add(_window, cx);
                                        });
                                    })
                            })
                            .when(!sync.auto_enabled, |button| {
                                button.opacity(0.45).cursor(CursorStyle::Arrow)
                            })
                            .child(div().text_size(px(14.)).text_color(accent).child("+"))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(text_2)
                                    .child(I18nKey::SyncAddBackend.text()),
                            ),
                    )
                    .when(!backend_cards.is_empty(), |card| {
                        card.child(
                            div()
                                .max_h(px(270.))
                                .overflow_y_scrollbar()
                                .p(px(8.))
                                .flex()
                                .flex_col()
                                .children(backend_cards),
                        )
                    }),
            )
    }

    fn render_backend_card(
        &mut self,
        backend: &BackendStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = backend.config.id.clone();
        let enabled = backend.config.enabled;
        let (transfer_station_enabled, transfer_selected) = {
            let settings = &self.state.read(cx).settings;
            let selected = settings
                .sync_backends
                .iter()
                .find(|config| {
                    config.enabled
                        && !settings.transfer_backend_id.is_empty()
                        && config.id == settings.transfer_backend_id
                })
                .or_else(|| settings.sync_backends.iter().find(|config| config.enabled))
                .is_some_and(|config| config.id == id);
            (settings.transfer_station_enabled, selected)
        };
        let (previous, generation, changed) = match self.backend_collapse_states.get_mut(&id) {
            Some(state) => {
                let previous = state.enabled;
                let changed = previous != enabled;
                if changed {
                    state.enabled = enabled;
                    state.generation = state.generation.wrapping_add(1);
                }
                (previous, state.generation, changed)
            }
            None => {
                self.backend_collapse_states.insert(
                    id.clone(),
                    BackendCollapseState {
                        enabled,
                        generation: 0,
                    },
                );
                (enabled, 0, false)
            }
        };
        let key = hash_key(&id).wrapping_add(generation << 32);
        let height = transition_f32(
            window,
            cx,
            ("sync-backend-height", key),
            if changed {
                card_height(previous)
            } else {
                card_height(enabled)
            },
            card_height(enabled),
        );
        let footer_opacity = transition_f32(
            window,
            cx,
            ("sync-backend-footer-opacity", key),
            if changed {
                bool_f32(previous)
            } else {
                bool_f32(enabled)
            },
            bool_f32(enabled),
        );
        let content_opacity = transition_f32(
            window,
            cx,
            ("sync-backend-content-opacity", key),
            if changed {
                main_opacity(previous)
            } else {
                main_opacity(enabled)
            },
            main_opacity(enabled),
        );

        let surface = self.theme.titlebar_bg;
        let divider = self.theme.divider;
        let accent = self.theme.accent;
        let accent_soft = self.theme.accent_soft;
        let text_1 = self.theme.text_1;
        let text_2 = self.theme.text_2;
        let text_3 = self.theme.text_3;
        let danger = self.theme.danger;
        let auto_enabled = self.state.read(cx).sync.auto_enabled;
        let wm = self.window_manager.clone();
        let backend_panel = self.backend_panel();
        let status_color = match backend.status.as_str() {
            "online" => rgb(0x4caf50),
            "syncing" => rgb(0x3b82f6),
            "error" => danger,
            _ => rgb(0x9e9e9e),
        };
        let interval = backend.config.sync_interval_secs.unwrap_or_else(|| {
            if backend.config.backend_type == "webdav" {
                600
            } else {
                60
            }
        });
        let stats = format!(
            "{} · {} {} · {} {}",
            format_last_sync(&backend.config.last_sync_at),
            backend.config.last_item_count,
            I18nKey::SyncStatsItems.text(),
            backend.config.last_tag_count,
            I18nKey::SyncStatsTags.text()
        );

        div()
            .mb(px(8.))
            .h(px(height))
            .rounded(px(8.))
            .border(px(1.))
            .border_color(divider)
            .bg(surface)
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(60.))
                    .flex_shrink_0()
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .opacity(content_opacity)
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_family("iconfont")
                                            .text_color(status_color)
                                            .flex_shrink_0()
                                            .child("\u{e61f}"),
                                    )
                                    .child(
                                        div()
                                            .min_w(px(0.))
                                            .flex_1()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(text_1)
                                            .child(backend.config.name.clone()),
                                    ),
                            )
                            .child(
                                div().flex().items_center().child(
                                    div()
                                        .h(px(16.))
                                        .max_w(px(150.))
                                        .px(px(5.))
                                        .rounded(px(3.))
                                        .bg(accent_soft)
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_size(px(10.))
                                        .text_color(accent)
                                        .flex()
                                        .items_center()
                                        .child(backend.service_label.clone()),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .opacity(content_opacity)
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.))
                                            .child(
                                                if auto_enabled
                                                    && enabled
                                                    && transfer_station_enabled
                                                {
                                                    icon_button(
                                                        ("sync-transfer", key),
                                                        "\u{e794}",
                                                        if transfer_selected {
                                                            accent
                                                        } else {
                                                            text_3
                                                        },
                                                        accent,
                                                        if transfer_selected {
                                                            I18nKey::BackendTooltipTransferActive
                                                                .text()
                                                        } else {
                                                            I18nKey::BackendTooltipSetTransfer
                                                                .text()
                                                        },
                                                        {
                                                            let id = id.clone();
                                                            let wm = wm.clone();
                                                            move |_window, cx| {
                                                                wm.update(cx, |wm, cx| {
                                                                    wm.set_transfer_backend(
                                                                        &id, cx,
                                                                    );
                                                                });
                                                            }
                                                        },
                                                    )
                                                    .into_any_element()
                                                } else {
                                                    disabled_icon_button(
                                                        ("sync-transfer", key),
                                                        "\u{e794}",
                                                        text_3,
                                                        if transfer_station_enabled {
                                                            I18nKey::BackendTooltipSetTransfer
                                                                .text()
                                                        } else {
                                                            I18nKey::BackendTooltipEnableTransfer
                                                                .text()
                                                        },
                                                    )
                                                    .into_any_element()
                                                },
                                            )
                                            .child(if auto_enabled {
                                                icon_button(
                                                    ("sync-edit", key),
                                                    "\u{e679}",
                                                    text_3,
                                                    accent,
                                                    I18nKey::BackendTooltipEdit.text(),
                                                    {
                                                        let config = backend.config.clone();
                                                        let backend_panel = backend_panel.clone();
                                                        move |window, cx| {
                                                            backend_panel.update(
                                                                cx,
                                                                |panel, cx| {
                                                                    panel.open_edit(
                                                                        &config, window, cx,
                                                                    );
                                                                },
                                                            );
                                                        }
                                                    },
                                                )
                                                .into_any_element()
                                            } else {
                                                disabled_icon_button(
                                                    ("sync-edit", key),
                                                    "\u{e679}",
                                                    text_3,
                                                    I18nKey::BackendTooltipEdit.text(),
                                                )
                                                .into_any_element()
                                            })
                                            .child(if auto_enabled {
                                                icon_button(
                                                    ("sync-delete", key),
                                                    "\u{e696}",
                                                    text_3,
                                                    danger,
                                                    I18nKey::BackendTooltipDelete.text(),
                                                    {
                                                        let id = id.clone();
                                                        let this = cx.entity().clone();
                                                        move |_window, cx| {
                                                            this.update(cx, |panel, cx| {
                                                            panel.delete_backend_confirm =
                                                                Some(id.clone());
                                                            panel.delete_backend_confirm_gen =
                                                                panel
                                                                    .delete_backend_confirm_gen
                                                                    .wrapping_add(1);
                                                            panel.delete_backend_confirm_started =
                                                                Some(std::time::Instant::now());
                                                            cx.notify();
                                                        });
                                                        }
                                                    },
                                                )
                                                .into_any_element()
                                            } else {
                                                disabled_icon_button(
                                                    ("sync-delete", key),
                                                    "\u{e696}",
                                                    text_3,
                                                    I18nKey::BackendTooltipDelete.text(),
                                                )
                                                .into_any_element()
                                            }),
                                    )
                                    .child(if auto_enabled {
                                        render_toggle(
                                            enabled,
                                            &format!("sync-backend-{id}"),
                                            ToggleColors {
                                                accent,
                                                track_off: divider,
                                            },
                                            &mut self.toggle_states,
                                            window,
                                            cx,
                                            {
                                                let id = id.clone();
                                                let wm = wm.clone();
                                                move |_window, cx| {
                                                    wm.update(cx, |wm, cx| {
                                                        wm.toggle_sync_backend(&id, cx);
                                                    });
                                                }
                                            },
                                        )
                                        .into_any_element()
                                    } else {
                                        disabled_toggle(enabled, divider).into_any_element()
                                    }),
                            )
                            .child(
                                div()
                                    .max_w(px(210.))
                                    .text_size(px(10.))
                                    .text_color(text_3)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(stats),
                            ),
                    ),
            )
            .child(
                div()
                    .h(px(31.))
                    .flex_shrink_0()
                    .opacity(footer_opacity)
                    .border_t(px(1.))
                    .border_color(divider)
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .children(
                        [
                            (30, I18nKey::SyncInterval30s.text()),
                            (60, I18nKey::SyncInterval1m.text()),
                            (600, I18nKey::SyncInterval10m.text()),
                            (1800, I18nKey::SyncInterval30m.text()),
                        ]
                        .into_iter()
                        .map(|(secs, label)| {
                            let selected = interval == secs;
                            let id = id.clone();
                            let wm = wm.clone();
                            div()
                                .flex_1()
                                .h(px(20.))
                                .rounded(px(6.))
                                .bg(if selected { accent } else { rgba(0x00000000) })
                                .text_size(px(10.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(if selected { rgb(0xffffff) } else { text_2 })
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor(CursorStyle::PointingHand)
                                .hover(move |style| {
                                    if selected {
                                        style.opacity(0.88)
                                    } else {
                                        style.bg(accent_soft)
                                    }
                                })
                                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                    wm.update(cx, |wm, cx| {
                                        wm.set_backend_sync_interval(&id, secs, cx);
                                    });
                                })
                                .child(label)
                        }),
                    )
                    .child(
                        div()
                            .w(px(62.))
                            .h(px(20.))
                            .rounded(px(6.))
                            .bg(accent)
                            .text_size(px(10.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0xffffff))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(|style| style.opacity(0.85))
                            .on_mouse_down(MouseButton::Left, {
                                let id = id.clone();
                                let wm = wm.clone();
                                move |_ev, _window, cx| {
                                    wm.update(cx, |wm, cx| {
                                        wm.sync_backend_now(&id, cx);
                                    });
                                }
                            })
                            .child(if backend.syncing {
                                I18nKey::SyncSyncing.text()
                            } else {
                                I18nKey::SyncNow.text()
                            }),
                    ),
            )
    }
}

fn icon_button(
    button_id: (&'static str, u64),
    icon: &'static str,
    color: Rgba,
    hover_color: Rgba,
    tooltip: &'static str,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(button_id)
        .w(px(24.))
        .h(px(24.))
        .rounded(px(5.))
        .font_family("iconfont")
        .text_size(px(12.))
        .text_color(color)
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::PointingHand)
        .hover(move |style| style.text_color(hover_color).bg(rgba(0xffffff0d)))
        .tooltip(move |window, cx| {
            Tooltip::element(move |_window, _cx| div().text_size(px(10.)).child(tooltip))
                .build(window, cx)
        })
        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            on_click(window, cx);
        })
        .child(icon)
}

fn disabled_icon_button(
    button_id: (&'static str, u64),
    icon: &'static str,
    color: Rgba,
    tooltip: &'static str,
) -> impl IntoElement {
    div()
        .id(button_id)
        .w(px(24.))
        .h(px(24.))
        .rounded(px(5.))
        .font_family("iconfont")
        .text_size(px(12.))
        .text_color(color)
        .opacity(0.35)
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::Arrow)
        .tooltip(move |window, cx| {
            Tooltip::element(move |_window, _cx| div().text_size(px(10.)).child(tooltip))
                .build(window, cx)
        })
        .child(icon)
}

fn disabled_toggle(value: bool, track_off: Rgba) -> impl IntoElement {
    let knob_x = if value { 20.0 } else { 2.0 };
    div()
        .w(px(40.))
        .h(px(22.))
        .rounded(px(11.))
        .bg(track_off)
        .opacity(0.45)
        .flex()
        .items_center()
        .cursor(CursorStyle::Arrow)
        .child(
            div()
                .w(px(18.))
                .h(px(18.))
                .rounded(px(9.))
                .bg(rgb(0xffffff))
                .ml(px(knob_x)),
        )
}

fn transition_f32(
    window: &mut Window,
    cx: &mut App,
    key: (&'static str, u64),
    initial: f32,
    target: f32,
) -> f32 {
    let transition = window
        .use_keyed_transition(key, cx, COLLAPSE_DURATION, move |_, _| initial)
        .with_easing(ease_out);
    transition.update(cx, |value, cx| {
        *value = target;
        cx.notify();
    });
    let value = *transition.evaluate(window, cx);
    value
}

fn hash_key(value: &str) -> u64 {
    value.bytes().fold(0u64, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as u64)
    })
}

fn card_height(enabled: bool) -> f32 {
    if enabled {
        91.0
    } else {
        60.0
    }
}

fn bool_f32(value: bool) -> f32 {
    if value {
        1.0
    } else {
        0.0
    }
}

fn main_opacity(enabled: bool) -> f32 {
    if enabled {
        1.0
    } else {
        0.45
    }
}

fn ease_out(delta: f32) -> f32 {
    1.0 - (1.0 - delta).powi(3)
}
