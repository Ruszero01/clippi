# GPUI Settings — General & Clipboard Tabs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate General and Clipboard settings tab content from Slint stubs to functional GPUI controls, wire all mutations to AppState, and fix the initial window position hardcode bug.

**Architecture:** SettingsPanel accepts `Entity<AppState>` and `Entity<WindowManager>`, reads current values from AppState for rendering, writes mutations back through AppState (which persists to disk). Two private helper methods (`setting_row_with_toggle`, `setting_row_with_options`) factor out repeated control patterns. Tab content delegates to `general.rs` and `clipboard.rs` sub-modules. A new `calculate_initial_position` free function in `core/frontend.rs` replaces the hardcoded `(100, 100)` in main.rs.

**Tech Stack:** Rust, GPUI, existing `AppSettings` (TOML-backed), `WindowManager` entity, `ClippiTheme`

---

### Task 1: Extract initial window position calculation

**Files:**
- Modify: `src/core/frontend.rs` (add function at end of file)
- Modify: `src/main.rs:75-80` (replace hardcoded position)

- [ ] **Step 1: Add `calculate_initial_position` to `core/frontend.rs`**

Append this function to `src/core/frontend.rs`:

```rust
/// Calculate the initial window position based on settings.
///
/// Returns `(x, y)` in physical pixels (Windows) or logical points (macOS),
/// or `None` if the monitor layout is unavailable (falls back to a safe default).
pub fn calculate_initial_position(settings: &crate::core::settings::AppSettings) -> Option<(i32, i32)> {
    let mode = PositionMode::from_str(&settings.window_position_mode);
    let win_w = if settings.saved_window_width > 0.0 {
        settings.saved_window_width.max(DEFAULT_WINDOW_WIDTH)
    } else {
        DEFAULT_WINDOW_WIDTH
    } as i32;
    let win_h = if settings.saved_window_height > 0.0 {
        settings.saved_window_height.max(DEFAULT_WINDOW_HEIGHT)
    } else {
        DEFAULT_WINDOW_HEIGHT
    } as i32;

    match mode {
        PositionMode::Center => calc_center(win_w, win_h),
        PositionMode::FollowMouse => calc_follow_mouse(win_w, win_h),
        PositionMode::Remember => calc_remember(settings, win_w, win_h)
            .or_else(|| calc_center(win_w, win_h)),
    }
}

fn calc_center(win_w: i32, win_h: i32) -> Option<(i32, i32)> {
    let (cx, cy) = monitor::get_cursor_pos()?;
    let area = monitor::get_monitor_work_area(cx, cy)?;
    let x = area.x + (area.width - win_w) / 2;
    let y = area.y + (area.height - win_h) / 2;
    Some((x, y))
}

fn calc_follow_mouse(win_w: i32, win_h: i32) -> Option<(i32, i32)> {
    let (cx, cy) = monitor::get_cursor_pos()?;
    let area = monitor::get_monitor_work_area(cx, cy)?;
    Some(clamp_to_work_area(
        cx - PANEL_OFFSET_X as i32,
        cy,
        win_w,
        win_h,
        &area,
    ))
}

fn calc_remember(
    settings: &crate::core::settings::AppSettings,
    win_w: i32,
    win_h: i32,
) -> Option<(i32, i32)> {
    let (sx, sy) = (settings.saved_window_x, settings.saved_window_y);
    if sx < 0 || sy < 0 {
        return None;
    }
    if !monitor::is_point_on_monitor(sx, sy) {
        return None;
    }
    let area = monitor::get_monitor_work_area(sx, sy)?;
    Some(clamp_to_work_area(sx, sy, win_w, win_h, &area))
}
```

- [ ] **Step 2: Update `main.rs` to use calculated position**

Replace lines 75-80 in `src/main.rs`:

```rust
// Before (hardcoded):
WindowOptions {
    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
        point(px(100.), px(100.)),
        size(px(360.), px(480.)),
    ))),
```

Replace with:

```rust
let settings = AppSettings::load();
let initial_pos = core::frontend::calculate_initial_position(&settings)
    .map(|(x, y)| point(px(x as f32), px(y as f32)))
    .unwrap_or(point(px(100.), px(100.)));
let initial_size = size(
    px(if settings.saved_window_width > 0.0 {
        settings.saved_window_width
    } else {
        core::frontend::DEFAULT_WINDOW_WIDTH
    }),
    px(if settings.saved_window_height > 0.0 {
        settings.saved_window_height
    } else {
        core::frontend::DEFAULT_WINDOW_HEIGHT
    }),
);
// ... later in cx.open_window:
WindowOptions {
    window_bounds: Some(WindowBounds::Windowed(Bounds::new(initial_pos, initial_size))),
```

Note: also replace `AppSettings::load()` on the later line where AppState is created with `settings.clone()` to avoid loading twice:

```rust
// Before:
let state = cx.new(|_cx| AppState::new(AppSettings::load()));
// After:
let state = cx.new(|_cx| AppState::new(settings.clone()));
```

- [ ] **Step 3: Build check**

```bash
cargo build 2>&1 | head -40
```

Expected: compiles successfully. If `core/frontend.rs` has a missing import for `AppSettings`, verify the full path `crate::core::settings::AppSettings` resolves — it should since `frontend.rs` is in `core/` and `settings.rs` is also in `core/`.

- [ ] **Step 4: Commit**

```bash
git add src/core/frontend.rs src/main.rs
git commit -m "fix: calculate initial window position from settings instead of hardcoded (100,100)

Extract calculate_initial_position() to core/frontend.rs supporting all
three PositionMode variants (Center/FollowMouse/Remember). Use it in
main.rs to set the initial window_bounds. Also use saved window size
if available."
```

---

### Task 2: Refactor SettingsPanel to accept Entity<AppState> and Entity<WindowManager>

**Files:**
- Modify: `src/ui/settings/mod.rs` (struct, constructor, remove `settings: AppSettings` field)

- [ ] **Step 1: Update SettingsPanel struct and constructor**

Replace the struct definition and `new()` in `src/ui/settings/mod.rs`:

```rust
use crate::state::app::AppState;
use crate::ui::window_manager::WindowManager;

/// Events emitted by the settings panel.
pub enum SettingsEvent {
    /// User clicked the back button — return to clipboard view.
    Back,
    /// Theme setting changed — RootView should rebuild its ClippiTheme.
    ThemeChanged(String),
}

impl EventEmitter<SettingsEvent> for SettingsPanel {}

/// The settings panel entity.
pub struct SettingsPanel {
    active_tab: usize,
    state: Entity<AppState>,
    window_manager: Entity<WindowManager>,
    theme: ClippiTheme,
}

const TAB_NAMES: &[&str] = &["General", "Clipboard", "Hotkey", "Data", "Sync"];

impl SettingsPanel {
    pub fn new(
        state: Entity<AppState>,
        window_manager: Entity<WindowManager>,
        cx: &mut Context<Self>,
    ) -> Self {
        let theme = {
            let settings = &state.read(cx).settings;
            ClippiTheme::from_setting(&settings.theme, None)
        };
        Self {
            active_tab: 0,
            state,
            window_manager,
            theme,
        }
    }

    pub fn set_tab(&mut self, tab: usize, cx: &mut Context<Self>) {
        self.active_tab = tab;
        cx.notify();
    }

    /// Reload theme from current AppState settings (called by RootView after ThemeChanged).
    pub fn reload_theme(&mut self, cx: &mut Context<Self>) {
        let new_theme = {
            let settings = &self.state.read(cx).settings;
            ClippiTheme::from_setting(&settings.theme, None)
        };
        self.theme = new_theme;
        cx.notify();
    }
}
```

- [ ] **Step 2: Remove unused `AppSettings` import if needed**

Check if `use crate::core::settings::AppSettings;` is still needed at the top of `mod.rs`. It will be needed for reference but not for the struct field anymore. Keep it — it's used in type annotations and the tab renderers will reference it.

- [ ] **Step 3: Commit**

```bash
git add src/ui/settings/mod.rs
git commit -m "refactor: SettingsPanel accepts Entity<AppState> + Entity<WindowManager>

Replace the owned AppSettings copy with entity references so settings
mutations take immediate effect and theme changes propagate to RootView.
Add ThemeChanged event variant and reload_theme() method."
```

---

### Task 3: Update RootView to wire new SettingsPanel and handle ThemeChanged

**Files:**
- Modify: `src/ui/root.rs:54` (SettingsPanel construction)
- Modify: `src/ui/root.rs:120-128` (SettingsEvent subscription)

- [ ] **Step 1: Update SettingsPanel construction in RootView::new()**

Replace line 54 in `src/ui/root.rs`:

```rust
// Before:
let settings_panel = cx.new(|cx| SettingsPanel::new(cx));
// After:
let settings_panel = cx.new(|cx| SettingsPanel::new(state.clone(), window_manager.clone(), cx));
```

- [ ] **Step 2: Update SettingsEvent handler to react to ThemeChanged**

Replace the settings subscription block (lines 120-128) in `src/ui/root.rs`:

```rust
// Before:
cx.subscribe(
    &settings_panel,
    move |this, _panel, event: &SettingsEvent, cx| match event {
        SettingsEvent::Back => {
            this.current_view = "clipboard".into();
            cx.notify();
        }
    },
),
// After:
cx.subscribe(
    &settings_panel,
    move |this, _panel, event: &SettingsEvent, cx| match event {
        SettingsEvent::Back => {
            this.current_view = "clipboard".into();
            cx.notify();
        }
        SettingsEvent::ThemeChanged(theme_str) => {
            let appearance = cx.window_appearance();
            this.theme = ClippiTheme::from_setting(theme_str, appearance);
            // Notify settings panel to rebuild with new theme colors
                            let _ = this.settings_panel.update(cx, |panel, cx| {
                                panel.reload_theme(cx);
                            });
                            cx.notify();
                        }
                    },
            ),
```

Wait — I need to be more careful with the exact structure. Let me use the actual closure pattern from the existing code:

```rust
            cx.subscribe(
                &settings_panel,
                move |this, _panel, event: &SettingsEvent, cx| match event {
                    SettingsEvent::Back => {
                        this.current_view = "clipboard".into();
                        cx.notify();
                    }
                    SettingsEvent::ThemeChanged(theme_str) => {
                        let appearance = cx.window_appearance();
                        this.theme = ClippiTheme::from_setting(theme_str, appearance);
                        let _ = this.settings_panel.update(cx, |panel, cx| {
                            panel.reload_theme(cx);
                        });
                        cx.notify();
                    }
                },
            ),
```

- [ ] **Step 3: Add `window_appearance` helper import if needed**

The `cx.window_appearance()` is available on `Context` in GPUI — no extra import needed. Verify `WindowAppearance` is importable from `gpui` — it's re-exported.

- [ ] **Step 4: Commit**

```bash
git add src/ui/root.rs
git commit -m "feat: wire SettingsPanel to AppState + WindowManager, handle ThemeChanged

RootView now passes AppState and WindowManager entities to SettingsPanel.
On ThemeChanged event, RootView rebuilds its ClippiTheme and notifies
the settings panel to re-render with new colors."
```

---

### Task 4: Add reusable control helpers to SettingsPanel

**Files:**
- Modify: `src/ui/settings/mod.rs` (add private methods after `render_*_tab` stubs)

- [ ] **Step 1: Add `setting_row_with_toggle` helper method**

Insert before the `render_general_tab` stub (or after the last render stub) in `src/ui/settings/mod.rs`:

```rust
impl SettingsPanel {
    /// Render a settings row with a toggle switch on the right.
    ///
    /// - `label`: setting name (12px, 600 weight, text_1)
    /// - `desc`: description text (10px, text_3)
    /// - `value`: current toggle state (true = on / accent, false = off / divider)
    /// - `on_toggle`: callback receiving the new value when clicked
    fn setting_row_with_toggle(
        &self,
        label: &str,
        desc: &str,
        value: bool,
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
            // Left: label + description
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(text_3)
                            .child(desc.to_string()),
                    ),
            )
            // Right: toggle switch (40×22px, 11px radius)
            .child(
                div()
                    .w(px(40.))
                    .h(px(22.))
                    .rounded(px(11.))
                    .bg(if value { accent } else { divider })
                    .px(px(2.))
                    .flex()
                    .items_center()
                    .when(value, |d| d.justify_end())
                    .when(!value, |d| d.justify_start())
                    .cursor(CursorStyle::PointingHand)
                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                        on_toggle(_window, cx);
                    })
                    .child(
                        // White circle knob (18×18px)
                        div()
                            .w(px(18.))
                            .h(px(18.))
                            .rounded(px(9.))
                            .bg(rgb(0xffffff)),
                    ),
            )
    }

    /// Render a settings row with an option button group on the right.
    ///
    /// - `label`: setting name (12px, 600 weight, text_1)
    /// - `desc`: description text (10px, text_3)
    /// - `options`: slice of `(key, display_label)` pairs; `key` is the internal value string
    /// - `active_key`: the currently selected option key
    /// - `on_select`: callback receiving the selected key when an option is clicked
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
            // Left: label + description
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(text_3)
                            .child(desc.to_string()),
                    ),
            )
            // Right: option buttons
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(4.))
                    .children(options.iter().map(|(key, display_label)| {
                        let selected = *key == active_key;
                        let btn_bg = if selected {
                            accent
                        } else {
                            rgba(0x00000000)
                        };
                        let btn_text = if selected {
                            rgb(0xffffff)
                        } else {
                            text_2
                        };
                        let btn_weight = if selected {
                            FontWeight::BOLD
                        } else {
                            FontWeight::default()
                        };
                        let key = *key;

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
}
```

Note: `rgb` and `rgba` are already imported from `gpui::*` at the top of the file. `FontWeight`, `CursorStyle`, `MouseButton` are also in the glob import.

- [ ] **Step 2: Commit**

```bash
git add src/ui/settings/mod.rs
git commit -m "feat: add setting_row_with_toggle and setting_row_with_options helpers

Two reusable private methods that render the standard 66px settings row
layout (surface bg, 10px radius, 1px divider border) with label+desc on
the left and either a toggle switch or option button group on the right."
```

---

### Task 5: Create settings/general.rs

**Files:**
- Create: `src/ui/settings/general.rs`

- [ ] **Step 1: Create `src/ui/settings/general.rs`**

Write the complete file:

```rust
//! General settings tab — language, startup, theme, position.
//!
//! Matches the original Slint `SettingsTabGeneral.slint` layout.
//! Language selector is rendered as UI but wired as a no-op pending
//! GPUI i18n implementation.

use gpui::*;

use crate::core::frontend::PositionMode;
use crate::core::settings::set_auto_start;
use crate::state::app::AppState;
use crate::ui::settings::SettingsEvent;
use crate::ui::theme::ClippiTheme;
use crate::ui::window_manager::WindowManager;

use super::SettingsPanel;

impl SettingsPanel {
    pub fn render_general_tab(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let wm = self.window_manager.clone();
        let this = cx.entity().clone();

        // Snapshot current values from AppState
        let app = self.state.read(cx);
        let auto_start = app.settings.auto_start;
        let auto_hide = app.settings.auto_hide;
        let silent_start = app.settings.silent_start;
        let theme_str = app.settings.theme.clone();
        let position_mode = app.settings.window_position_mode.clone();
        let language = app.settings.language.clone();
        drop(app);

        // Derive display indices from string settings
        let theme_idx = match theme_str.as_str() {
            "dark" => 1,
            "light" => 2,
            _ => 0, // "system" or unknown → Auto
        };
        let position_idx = match position_mode.as_str() {
            "follow" => 1,
            "remember" => 2,
            _ => 0, // "center" or unknown → Center
        };
        let lang_idx = if language == "en" { 1 } else { 0 };

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            // ── Language (placeholder — no-op callback) ──
            .child(self.setting_row_with_options(
                "Language",
                "Interface language",
                &[("zh", "中文"), ("en", "English")],
                if lang_idx == 1 { "en" } else { "zh" },
                {
                    // TODO: Wire up when GPUI i18n is implemented.
                    // Currently a no-op — the UI renders but clicking does nothing.
                    move |_key, _window, _cx| {}
                },
            ))
            // ── Auto-start ──
            .child({
                let state = state.clone();
                self.setting_row_with_toggle(
                    "Auto-start",
                    "Run on system startup",
                    auto_start,
                    move |_window, _cx| {
                        let new_val = state.update(_cx, |s, _cx| {
                            s.settings.auto_start = !s.settings.auto_start;
                            s.settings.auto_start
                        });
                        if let Err(e) = set_auto_start(new_val) {
                            log::error!("Failed to set auto-start: {e}");
                        }
                        // Revert on failure? The settings value is already saved.
                        // For simplicity, keep the setting as-is — the registry/plist
                        // operation is best-effort.
                        state.update(_cx, |s, _cx| s.settings.save());
                    }
                )
            })
            // ── Auto-hide ──
            .child({
                let state = state.clone();
                let wm = wm.clone();
                let this = this.clone();
                self.setting_row_with_toggle(
                    "Auto-hide",
                    "Hide on focus loss",
                    auto_hide,
                    move |_window, _cx| {
                        let new_val = state.update(_cx, |s, _cx| {
                            s.settings.auto_hide = !s.settings.auto_hide;
                            s.settings.save();
                            s.settings.auto_hide
                        });
                        wm.update(_cx, |wm, _cx| wm.set_auto_hide(new_val));
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // ── Silent start ──
            .child({
                let state = state.clone();
                self.setting_row_with_toggle(
                    "Silent start",
                    "Start silently in tray",
                    silent_start,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.silent_start = !s.settings.silent_start;
                            s.settings.save();
                        });
                    },
                )
            })
            // ── Theme ──
            .child({
                let state = state.clone();
                let this = this.clone();
                self.setting_row_with_options(
                    "Theme",
                    "Select theme",
                    &[("system", "Auto"), ("dark", "Dark"), ("light", "Light")],
                    match theme_idx {
                        1 => "dark",
                        2 => "light",
                        _ => "system",
                    },
                    move |key, _window, _cx| {
                        let theme_str = key.to_string();
                        state.update(_cx, |s, _cx| {
                            s.settings.theme = theme_str.clone();
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| {
                            cx.emit(SettingsEvent::ThemeChanged(theme_str));
                            cx.notify();
                        });
                    },
                )
            })
            // ── Window position ──
            .child({
                let state = state.clone();
                let wm = wm.clone();
                self.setting_row_with_options(
                    "Position",
                    "Popup position",
                    &[("center", "Center"), ("follow", "Follow"), ("remember", "Pin")],
                    match position_idx {
                        1 => "follow",
                        2 => "remember",
                        _ => "center",
                    },
                    move |key, _window, _cx| {
                        let mode = PositionMode::from_str(key);
                        state.update(_cx, |s, _cx| {
                            s.settings.window_position_mode = key.to_string();
                            s.settings.save();
                        });
                        wm.update(_cx, |wm, _cx| wm.set_position_mode(mode));
                    },
                )
            })
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/ui/settings/general.rs
git commit -m "feat: implement General settings tab with toggle/option controls

5 active settings: auto-start, auto-hide, silent-start, theme, position.
Language selector rendered with no-op callback (TODO for GPUI i18n).
All mutations write through AppState + save to disk. Auto-hide and
position changes propagate to WindowManager. Theme change emits
ThemeChanged event for RootView to rebuild ClippiTheme."
```

---

### Task 6: Create settings/clipboard.rs

**Files:**
- Create: `src/ui/settings/clipboard.rs`

- [ ] **Step 1: Create `src/ui/settings/clipboard.rs`**

Write the complete file:

```rust
//! Clipboard settings tab — sort, card height, source app, scroll, copy, hover, OCR, QR.
//!
//! Matches the original Slint `SettingsTabClipboard.slint` layout.

use gpui::*;

use crate::state::app::AppState;

use super::SettingsPanel;

impl SettingsPanel {
    pub fn render_clipboard_tab(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let this = cx.entity().clone();

        // Snapshot current values from AppState
        let app = self.state.read(cx);
        let sort_by_created = app.settings.sort_by_created;
        let card_height_mode = app.settings.card_height_mode.clone();
        let show_source_app = app.settings.show_source_app;
        let auto_scroll_to_top = app.settings.auto_scroll_to_top;
        let copy_as_plain_text = app.settings.copy_as_plain_text;
        let show_original_on_hover = app.settings.show_original_on_hover;
        let ocr_enabled = app.settings.ocr_enabled;
        let qr_enabled = app.settings.qr_enabled;
        drop(app);

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            // ── Sort by created ──
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if sort_by_created {
                    "First created"
                } else {
                    "Last modified"
                };
                self.setting_row_with_toggle(
                    "Sort by created",
                    desc,
                    sort_by_created,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.sort_by_created = !s.settings.sort_by_created;
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // ── Card height ──
            .child({
                let state = state.clone();
                self.setting_row_with_options(
                    "Card height",
                    "Adjust card height",
                    &[
                        ("high", "Tall"),
                        ("medium", "Med"),
                        ("low", "Short"),
                        ("auto", "Auto"),
                    ],
                    &card_height_mode,
                    move |key, _window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.card_height_mode = key.to_string();
                            s.settings.save();
                        });
                    },
                )
            })
            // ── Show source app ──
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if show_source_app {
                    "Show source app icon"
                } else {
                    "Show content type only"
                };
                self.setting_row_with_toggle(
                    "Show source app",
                    desc,
                    show_source_app,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.show_source_app = !s.settings.show_source_app;
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // ── Scroll to top ──
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if auto_scroll_to_top {
                    "Scroll to top on open"
                } else {
                    "Keep last scroll position"
                };
                self.setting_row_with_toggle(
                    "Scroll to top",
                    desc,
                    auto_scroll_to_top,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.auto_scroll_to_top = !s.settings.auto_scroll_to_top;
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // ── Copy as plain text ──
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if copy_as_plain_text {
                    "Save as plain text only"
                } else {
                    "Keep rich formatting"
                };
                self.setting_row_with_toggle(
                    "Copy as plain text",
                    desc,
                    copy_as_plain_text,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.copy_as_plain_text = !s.settings.copy_as_plain_text;
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // ── Show original on hover ──
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if show_original_on_hover {
                    "Show original on hover"
                } else {
                    "Cards with notes show note"
                };
                self.setting_row_with_toggle(
                    "Show original on hover",
                    desc,
                    show_original_on_hover,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.show_original_on_hover =
                                !s.settings.show_original_on_hover;
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // ── Auto Image OCR ──
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if ocr_enabled {
                    "Auto OCR for images"
                } else {
                    "OCR disabled"
                };
                self.setting_row_with_toggle(
                    "Auto Image OCR",
                    desc,
                    ocr_enabled,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.ocr_enabled = !s.settings.ocr_enabled;
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // ── Auto QR Detection ──
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if qr_enabled {
                    "Auto detect QR in images"
                } else {
                    "QR detection disabled"
                };
                self.setting_row_with_toggle(
                    "Auto QR Detection",
                    desc,
                    qr_enabled,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.qr_enabled = !s.settings.qr_enabled;
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/ui/settings/clipboard.rs
git commit -m "feat: implement Clipboard settings tab with toggle/option controls

8 settings: sort-by-created, card-height-mode, show-source-app,
scroll-to-top, copy-as-plain-text, show-original-on-hover, OCR, QR.
Each toggle has dynamic description text reflecting current state.
All mutations write through AppState + save to disk."
```

---

### Task 7: Update settings/mod.rs — delegate rendering, register modules, add scroll

**Files:**
- Modify: `src/ui/settings/mod.rs` (declarations, imports, render routing, stubs replacement)

- [ ] **Step 1: Add module declarations to `src/ui/settings/mod.rs`**

Add at the top of the file, after the existing imports:

```rust
mod clipboard;
mod general;
```

Place these between the existing `use` block and the `SettingsEvent` enum.

- [ ] **Step 2: Update render routing to pass Window and Context to tab methods**

Replace the tab content routing (the `match active` block around line 172) in the `Render` impl:

```rust
            // ── Tab content (fills remaining space, scrollable) ──
            .child(
                div()
                    .flex_1()
                    .px(px(8.))
                    .overflow_y_scroll()
                    .child(match active {
                        0 => self.render_general_tab(window, cx).into_any_element(),
                        1 => self.render_clipboard_tab(window, cx).into_any_element(),
                        2 => self.render_hotkey_tab().into_any_element(),
                        3 => self.render_data_tab().into_any_element(),
                        4 => self.render_sync_tab().into_any_element(),
                        _ => div().into_any_element(),
                    }),
            )
```

Key changes from the original:
- `.overflow_y_scroll()` added to the tab content container for scroll support
- `render_general_tab` and `render_clipboard_tab` now receive `(window, cx)` parameters
- The other stubs remain unchanged (they are still placeholders)

- [ ] **Step 3: Remove old stub implementations**

Delete the old `render_general_tab` and `render_clipboard_tab` stub methods (the placeholder divs in the `impl SettingsPanel` block at the bottom). Keep the other three stubs (`render_hotkey_tab`, `render_data_tab`, `render_sync_tab`).

The final stub section should look like:

```rust
// ── Tab rendering stubs (not yet migrated) ──

impl SettingsPanel {
    fn render_hotkey_tab(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(100.))
            .text_color(self.theme.text_3)
            .text_size(px(13.))
            .child("Hotkey settings")
    }

    fn render_data_tab(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(100.))
            .text_color(self.theme.text_3)
            .text_size(px(13.))
            .child("Data settings")
    }

    fn render_sync_tab(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(100.))
            .text_color(self.theme.text_3)
            .text_size(px(13.))
            .child("Sync settings")
    }
}
```

- [ ] **Step 4: Commit**

```bash
git add src/ui/settings/mod.rs
git commit -m "feat: delegate General/Clipboard tab rendering to sub-modules

Register general.rs and clipboard.rs as sub-modules. Update render
routing to pass Window + Context to tab methods. Add overflow_y_scroll
to tab content container. Remove old placeholder stubs for the two
implemented tabs."
```

---

### Task 8: Build and fix compilation errors

**Files:**
- All files modified/created above (error fix pass)

- [ ] **Step 1: Build**

```bash
cargo build 2>&1
```

Expected: may have compilation errors related to:
- Missing imports in general.rs or clipboard.rs
- `Window` parameter not available in `Render` trait — check if `window` needs to be passed through
- Closure capture issues with `'static` bounds

- [ ] **Step 2: Fix common issues**

If `Render::render()` doesn't provide `&mut Window` in the signature: the GPUI `Render` trait's `render` method signature is:
```rust
fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement
```
So `window` IS available. Pass it through: `self.render_general_tab(window, cx)`.

If closure `'static` issues arise with `on_mouse_down`: wrap entities in closures with `move` (already done in our code — `state.clone()`, `wm.clone()`, `this.clone()` are all captured by value before the closure).

If `SettingsEvent::ThemeChanged` shows "variant not defined": verify the enum was updated in Task 2 Step 1.

If `WindowManager::set_position_mode` or `set_auto_hide` type errors: these take `&mut self` and are called via `wm.update(_cx, |wm, _cx| wm.set_auto_hide(val))` — this should work.

- [ ] **Step 3: Fix and rebuild until clean**

Iterate on any remaining errors. Run `cargo build 2>&1` after each fix.

- [ ] **Step 4: Verify zero warnings**

```bash
cargo clippy 2>&1
```

Expected: zero warnings (per project convention). If any warnings appear, fix them.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "fix: resolve compilation errors for settings migration"
```

---

### Task 9: Final integration verification

**Files:**
- All (read-only verification)

- [ ] **Step 1: Run the application**

```bash
cargo run
```

Manual verification checklist:
1. Window appears at correct position (not always top-left) on first launch
2. Click settings gear → General tab renders with all 6 settings rows
3. Toggle "Auto-hide" → window should hide on focus loss per the new setting
4. Change "Theme" to Light → UI should switch to light theme immediately
5. Change "Position" to Follow → next show should position near cursor
6. Switch to Clipboard tab → all 8 settings rows render
7. Toggle "Sort by created" → description changes dynamically
8. Change "Card height" → options highlight correctly
9. Close and reopen app → all changed settings persist

- [ ] **Step 2: Commit any final fixes**

```bash
git add -A
git commit -m "chore: final adjustments after manual verification"
```
