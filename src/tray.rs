use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

pub enum TrayAction {
    Show,
    Quit,
}

pub struct TrayManager {
    _tray: TrayIcon,
    show_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
}

impl TrayManager {
    pub fn new() -> Self {
        let icon = create_icon();

        let menu = Menu::new();
        let show_item = MenuItem::new("显示窗口", true, None);
        let quit_item = MenuItem::new("退出", true, None);

        let show_id = show_item.id().clone();
        let quit_id = quit_item.id().clone();

        menu.append_items(&[&show_item, &quit_item]).unwrap();

        let tray = TrayIconBuilder::new()
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_tooltip("Clippi - 剪贴板管理器")
            .build()
            .unwrap();

        Self {
            _tray: tray,
            show_id,
            quit_id,
        }
    }

    pub fn poll_events(&self) -> Option<TrayAction> {
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick { .. } = event {
                return Some(TrayAction::Show);
            }
        }

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.show_id {
                return Some(TrayAction::Show);
            }
            if event.id == self.quit_id {
                return Some(TrayAction::Quit);
            }
        }

        None
    }
}

fn create_icon() -> Icon {
    let icon_bytes = include_bytes!("../assets/LOGO_notext.ico");
    let img = image::load_from_memory(icon_bytes).expect("Failed to load logo icon");
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), width, height).expect("Failed to create icon from RGBA")
}
