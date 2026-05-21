//! System tray - platform implementation using tray-icon

use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

/// Tray action events
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrayAction {
    Show,
    OpenSettings,
    Restart,
    Quit,
}

/// System tray manager
pub struct TrayManager {
    #[allow(dead_code)]
    tray: TrayIcon,
    /// Keep MenuItem objects alive — dropping them may invalidate
    /// internal muda references and cause menu text to disappear.
    #[allow(dead_code)]
    _items: [MenuItem; 4],
    show_id: tray_icon::menu::MenuId,
    settings_id: tray_icon::menu::MenuId,
    restart_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
}

impl TrayManager {
    pub fn new() -> Self {
        let icon = create_icon();

        let menu = Menu::new();
        let show_item = MenuItem::new("显示窗口", true, None);
        let settings_item = MenuItem::new("设置", true, None);
        let restart_item = MenuItem::new("重启应用", true, None);
        let quit_item = MenuItem::new("退出", true, None);

        let show_id = show_item.id().clone();
        let settings_id = settings_item.id().clone();
        let restart_id = restart_item.id().clone();
        let quit_id = quit_item.id().clone();

        menu.append_items(&[&show_item, &settings_item, &restart_item, &quit_item]).unwrap();

        let tray = TrayIconBuilder::new()
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_tooltip("Clippi")
            .build()
            .unwrap();

        Self {
            tray,
            _items: [show_item, settings_item, restart_item, quit_item],
            show_id,
            settings_id,
            restart_id,
            quit_id,
        }
    }

    /// Poll for tray events - call this from main thread
    pub fn poll(&self) -> Option<TrayAction> {
        // Check double-click
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick { .. } = event {
                return Some(TrayAction::Show);
            }
        }

        // Check menu events
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
