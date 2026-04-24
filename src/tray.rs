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
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);

    for y in 0..size {
        for x in 0..size {
            let in_board = x >= 6 && x < 26 && y >= 10 && y < 29;
            let in_clip = x >= 11 && x < 21 && y >= 5 && y < 12;

            if in_clip {
                rgba.extend_from_slice(&[100, 100, 100, 255]);
            } else if in_board {
                rgba.extend_from_slice(&[59, 130, 246, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    Icon::from_rgba(rgba, size, size).unwrap()
}
