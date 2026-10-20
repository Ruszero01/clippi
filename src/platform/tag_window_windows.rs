use std::{cell::Cell, rc::Rc};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{
            DwmSetWindowAttribute, DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
            DWMWA_NCRENDERING_POLICY, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
        },
        Gdi::ClientToScreen,
    },
    UI::{
        HiDpi::GetDpiForWindow,
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};

use super::{client_geometry, sidebar_height, window_size, SHADOW, TOP};
use crate::core::frontend::PANEL_MARGIN;

const SUBCLASS: usize = 0x434C_5447;
const SYNC: u32 = WM_APP + 0x321;

pub struct Attachment {
    main: HWND,
    sidebar: HWND,
    enabled: Cell<bool>,
    rows: Cell<usize>,
    pending: Cell<bool>,
    placed: Cell<Option<(i32, i32, i32, i32)>>,
}

impl Attachment {
    /// Both handles belong to GPUI windows on the calling UI thread.
    pub fn install(main: isize, sidebar: isize) -> Result<Rc<Self>, &'static str> {
        let attachment = Rc::new(Self {
            main: main as HWND,
            sidebar: sidebar as HWND,
            enabled: Cell::new(false),
            rows: Cell::new(0),
            pending: Cell::new(false),
            placed: Cell::new(None),
        });
        let data = Rc::as_ptr(&attachment) as usize;
        // SAFETY: the Rc remains alive until both subclasses are removed by Drop.
        unsafe {
            if SetWindowSubclass(attachment.main, Some(parent_proc), SUBCLASS, data) == 0
                || SetWindowSubclass(attachment.sidebar, Some(sidebar_proc), SUBCLASS, data) == 0
            {
                return Err("could not install tag window attachment");
            }
            SetWindowLongPtrW(attachment.sidebar, GWLP_HWNDPARENT, main);
            SetWindowLongW(attachment.sidebar, GWL_STYLE, WS_POPUP as i32);
            let style = GetWindowLongW(attachment.sidebar, GWL_EXSTYLE);
            SetWindowLongW(
                attachment.sidebar,
                GWL_EXSTYLE,
                (style | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as i32) & !(WS_EX_APPWINDOW as i32),
            );
            // This surface only draws tag pixels next to the owner's rounded
            // stroke. A DWM frame, border or corner clip would show around the
            // strip and read as a second, mismatched outline.
            let size = std::mem::size_of::<u32>() as u32;
            let no_frame = DWMNCRP_DISABLED as u32;
            DwmSetWindowAttribute(
                attachment.sidebar,
                DWMWA_NCRENDERING_POLICY as u32,
                &no_frame as *const _ as _,
                size,
            );
            let no_border = DWMWA_COLOR_NONE;
            DwmSetWindowAttribute(
                attachment.sidebar,
                DWMWA_BORDER_COLOR as u32,
                &no_border as *const _ as _,
                size,
            );
            let no_rounding = DWMWCP_DONOTROUND as u32;
            DwmSetWindowAttribute(
                attachment.sidebar,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                &no_rounding as *const _ as _,
                size,
            );
        }
        Ok(attachment)
    }

    pub fn set_enabled(&self, enabled: bool) {
        if self.enabled.replace(enabled) != enabled {
            self.request_sync();
        }
    }

    pub fn set_rows(&self, rows: usize) {
        if self.rows.replace(rows) != rows {
            self.request_sync();
        }
    }

    pub fn available_height(&self) -> f32 {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: read-only query on our own window.
        unsafe {
            GetClientRect(self.main, &mut rect);
            ((rect.bottom - rect.top) as f32 / self.scale_factor() - PANEL_MARGIN * 2.).max(0.)
        }
    }

    pub fn refresh(&self) {
        self.request_sync();
    }

    fn request_sync(&self) {
        // A native queue message is independent of GPUI frame throttling. It also
        // avoids entering another GPUI window callback while AppCell is borrowed.
        if !self.pending.replace(true) {
            unsafe { PostMessageW(self.sidebar, SYNC, 0, 0) };
        }
    }

    fn sync(&self) {
        self.pending.set(false);
        if !self.enabled.get()
            || unsafe { IsWindowVisible(self.main) } == 0
            || unsafe { IsIconic(self.main) } != 0
            // A maximized window fills the work area, so there is no room beside it.
            || unsafe { IsZoomed(self.main) } != 0
        {
            self.hide();
            return;
        }
        let Some(origin) = self.client_origin() else {
            return;
        };
        self.place(origin, self.available_height());
    }

    /// Follow the owner inside the native modal move/resize loop.
    ///
    /// USER32 owns that loop, so no GPUI entity update is on the stack and both
    /// windows can commit the same rectangle in one frame. Going through the
    /// posted [`SYNC`] message instead makes the tag surface arrive one queue turn
    /// later, which is what produced the trailing "jelly" motion while dragging.
    fn follow(&self, proposed: &RECT) {
        if !self.enabled.get() {
            return;
        }
        let Some((origin_offset, vertical_inset)) = self.frame_insets() else {
            return;
        };
        let (client, client_height) = client_geometry(
            (proposed.left, proposed.top, proposed.right, proposed.bottom),
            origin_offset,
            vertical_inset,
        );
        self.place(
            POINT {
                x: client.0,
                y: client.1,
            },
            (client_height as f32 / self.scale_factor() - PANEL_MARGIN * 2.).max(0.),
        );
    }

    /// Position and size the surface so its right edge meets the content panel's
    /// left edge, which sits `PANEL_MARGIN` inside the owner's client origin.
    fn place(&self, client: POINT, panel_height: f32) -> bool {
        let strip = sidebar_height(panel_height, self.rows.get());
        if !self.enabled.get() || strip == 0. {
            self.hide();
            return false;
        }
        let scale = self.scale_factor();
        let (logical_width, logical_height) = window_size(strip);
        let width = (logical_width * scale).round() as i32;
        let height = (logical_height * scale).round() as i32;
        let x = client.x + (PANEL_MARGIN * scale).round() as i32 - width;
        // The strip starts `SHADOW` below the surface's top edge, `TOP` below the
        // panel's top edge.
        let y = client.y + ((PANEL_MARGIN + TOP - SHADOW) * scale).round() as i32;
        let target = (x, y, width, height);
        // SAFETY: the message-only checks and the move below act on our own windows.
        unsafe {
            if self.placed.get() == Some(target) && IsWindowVisible(self.sidebar) != 0 {
                return true;
            }
            let moved = SetWindowPos(
                self.sidebar,
                std::ptr::null_mut(),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOCOPYBITS | SWP_SHOWWINDOW,
            ) != 0;
            if moved {
                self.placed.set(Some(target));
            }
            moved
        }
    }

    fn hide(&self) {
        self.placed.set(None);
        // SAFETY: hiding our own window.
        unsafe { ShowWindow(self.sidebar, SW_HIDE) };
    }

    fn client_origin(&self) -> Option<POINT> {
        let mut origin = POINT { x: 0, y: 0 };
        // SAFETY: read-only query on our own window.
        if unsafe { ClientToScreen(self.main, &mut origin) } == 0 {
            None
        } else {
            Some(origin)
        }
    }

    /// Client origin relative to the window rectangle plus the vertical frame
    /// inset. Both stay constant during a move or resize, which lets the tag
    /// surface be placed from the rectangle USER32 is about to commit.
    fn frame_insets(&self) -> Option<((i32, i32), i32)> {
        let mut window = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let mut client = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let origin = self.client_origin()?;
        // SAFETY: read-only queries on our own window.
        unsafe {
            if GetWindowRect(self.main, &mut window) == 0
                || GetClientRect(self.main, &mut client) == 0
            {
                return None;
            }
        }
        Some((
            (origin.x - window.left, origin.y - window.top),
            (window.bottom - window.top) - (client.bottom - client.top),
        ))
    }

    fn scale_factor(&self) -> f32 {
        // SAFETY: read-only query on our own window.
        (unsafe { GetDpiForWindow(self.main) } as f32 / 96.).max(1.)
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        unsafe {
            RemoveWindowSubclass(self.main, Some(parent_proc), SUBCLASS);
            RemoveWindowSubclass(self.sidebar, Some(sidebar_proc), SUBCLASS);
        }
    }
}

unsafe extern "system" fn parent_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    let attachment = unsafe { &*(data as *const Attachment) };
    match message {
        // Interactive move and resize loops. `lparam` points at the rectangle
        // USER32 is about to commit, so the tag surface can move first.
        WM_MOVING | WM_SIZING => {
            if let Some(proposed) = unsafe { (lparam as *const RECT).as_ref() } {
                attachment.follow(proposed);
            }
        }
        // Everything else can also arrive from inside a GPUI entity update, where
        // touching the tag window's window procedure would re-enter the app cell.
        // Those paths stay on the posted sync message.
        WM_WINDOWPOSCHANGED | WM_SIZE | WM_DPICHANGED | WM_SHOWWINDOW | WM_EXITSIZEMOVE => {
            attachment.request_sync();
        }
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

unsafe extern "system" fn sidebar_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    match message {
        SYNC => {
            unsafe { &*(data as *const Attachment) }.sync();
            0
        }
        // GPUI's frameless calculation reserves a system frame even on popups.
        // This surface has only tag pixels, with no resize or nonclient border.
        WM_NCCALCSIZE => 0,
        WM_NCHITTEST => HTCLIENT as LRESULT,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_POINTERACTIVATE => PA_NOACTIVATE as LRESULT,
        // The attachment owns display migration, not GPUI's activating fallback.
        WM_DISPLAYCHANGE => 0,
        _ => unsafe { DefSubclassProc(hwnd, message, wparam, lparam) },
    }
}
