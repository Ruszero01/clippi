//! --- System tray - platform implementation using tray-icon ---

use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

/// Tray action events
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrayAction {
    Show,
    OpenSettings,
    Restart,
    Quit,
    CheckUpdate,
}

use crate::core::i18n;

/// System tray manager
pub struct TrayManager {
    #[allow(dead_code)]
    tray: TrayIcon,
    /// Keep menu item objects alive — dropping them may invalidate
    /// internal muda references and cause menu text to disappear.
    #[allow(dead_code)]
    _version_item: MenuItem,
    #[allow(dead_code)]
    _check_update_item: MenuItem,
    #[allow(dead_code)]
    _sep: PredefinedMenuItem,
    #[allow(dead_code)]
    _items: [MenuItem; 4],
    check_update_id: tray_icon::menu::MenuId,
    show_id: tray_icon::menu::MenuId,
    settings_id: tray_icon::menu::MenuId,
    restart_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
}

impl TrayManager {
    pub fn new() -> Self {
        let icon = create_icon();

        let menu = Menu::new();

        // --- Version label (disabled, gray) ---
        let version_text = format!("Clippi v{}", env!("CARGO_PKG_VERSION"));
        let version_item = MenuItem::new(&version_text, false, None);

        // --- Separator between version and functional items ---
        let sep = PredefinedMenuItem::separator();

        // Check for updates button
        let check_update_item =
            MenuItem::new(i18n::tr("检查更新", "Check for Updates"), true, None);
        let check_update_id = check_update_item.id().clone();

        // --- Existing functional menu items ---
        let show_item = MenuItem::new(i18n::tr("显示窗口", "Show Window"), true, None);
        let settings_item = MenuItem::new(i18n::tr("设置", "Settings"), true, None);
        let restart_item = MenuItem::new(i18n::tr("重启应用", "Restart"), true, None);
        let quit_item = MenuItem::new(i18n::tr("退出", "Quit"), true, None);

        let show_id = show_item.id().clone();
        let settings_id = settings_item.id().clone();
        let restart_id = restart_item.id().clone();
        let quit_id = quit_item.id().clone();

        menu.append_items(&[
            &version_item,
            &sep,
            &check_update_item,
            &show_item,
            &settings_item,
            &restart_item,
            &quit_item,
        ])
        .unwrap();

        let tray = TrayIconBuilder::new()
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_tooltip("Clippi")
            .build()
            .unwrap();

        Self {
            tray,
            _version_item: version_item,
            _check_update_item: check_update_item,
            _sep: sep,
            _items: [show_item, settings_item, restart_item, quit_item],
            check_update_id,
            show_id,
            settings_id,
            restart_id,
            quit_id,
        }
    }

    /// Poll for tray events - call this from main thread
    pub fn poll(&self) -> Option<TrayAction> {
        // --- Check double-click ---
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick { .. } = event {
                return Some(TrayAction::Show);
            }
        }

        // --- Check menu events ---
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.show_id {
                return Some(TrayAction::Show);
            }
            if event.id == self.settings_id {
                return Some(TrayAction::OpenSettings);
            }
            if event.id == self.restart_id {
                return Some(TrayAction::Restart);
            }
            if event.id == self.quit_id {
                return Some(TrayAction::Quit);
            }
            if event.id == self.check_update_id {
                return Some(TrayAction::CheckUpdate);
            }
        }

        None
    }
}

fn create_icon() -> Icon {
    #[cfg(target_os = "windows")]
    let icon_bytes = include_bytes!("../../assets/LOGO_notext.ico");
    #[cfg(target_os = "macos")]
    let icon_bytes = include_bytes!("../../assets/LOGO_notext.png");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let icon_bytes = include_bytes!("../../assets/LOGO_notext.png");

    let img = image::load_from_memory(icon_bytes).expect("Failed to load logo icon");
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), width, height).expect("Failed to create icon from RGBA")
}
