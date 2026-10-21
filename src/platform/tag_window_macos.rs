use std::{cell::Cell, rc::Rc, sync::OnceLock};

use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{define_class, msg_send, rc::Retained, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSColor, NSWindow, NSWindowDidBecomeKeyNotification, NSWindowDidDeminiaturizeNotification,
    NSWindowDidResizeNotification, NSWindowOrderingMode,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol,
    NSPoint, NSRect, NSRunLoopCommonModes, NSSize,
};

use super::{sidebar_height, window_size, SHADOW, TOP};
use crate::core::frontend::PANEL_MARGIN;

pub struct Attachment {
    observer: Retained<TagWindowObserver>,
}

struct AttachmentState {
    main: Retained<NSWindow>,
    sidebar: Retained<NSWindow>,
    enabled: Cell<bool>,
    rows: Cell<usize>,
    pending: Cell<bool>,
}

unsafe extern "C" fn cannot_become_key(_: *mut AnyObject, _: Sel) -> Bool {
    Bool::NO
}

/// GPUI's GPUIPanel overrides canBecomeKeyWindow to YES. Restrict only this
/// instance using an ivar-free subclass; do not change GPUI's shared panel class.
fn nonactivating_class(panel: &NSWindow) -> &'static AnyClass {
    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| {
        let mut builder =
            ClassBuilder::new(c"ClippiTagPanel", panel.class()).expect("unique tag panel class");
        unsafe {
            builder.add_method(
                sel!(canBecomeKeyWindow),
                cannot_become_key as unsafe extern "C" fn(*mut AnyObject, Sel) -> Bool,
            );
            builder.add_method(
                sel!(canBecomeMainWindow),
                cannot_become_key as unsafe extern "C" fn(*mut AnyObject, Sel) -> Bool,
            );
        }
        builder.register()
    })
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = AttachmentState]
    struct TagWindowObserver;

    unsafe impl NSObjectProtocol for TagWindowObserver {}

    impl TagWindowObserver {
        #[unsafe(method(parentGeometryChanged:))]
        fn parent_geometry_changed(&self, _notification: &NSNotification) {
            self.schedule();
        }

        #[unsafe(method(syncSidebar))]
        fn sync_sidebar(&self) {
            self.ivars().pending.set(false);
            self.sync();
        }
    }
);

impl TagWindowObserver {
    fn schedule(&self) {
        if self.ivars().pending.replace(true) {
            return;
        }
        // Run outside GPUI's AppCell borrow, including during live resize's
        // event-tracking run loop. Ordinary movement is owned by addChildWindow
        // and needs neither this callback nor a GPUI redraw.
        unsafe {
            let modes = NSArray::from_slice(&[NSRunLoopCommonModes]);
            let _: () = msg_send![self,
                performSelector: sel!(syncSidebar),
                withObject: std::ptr::null::<NSObject>(),
                afterDelay: 0.0f64,
                inModes: &*modes
            ];
        }
    }

    fn sync(&self) {
        let state = self.ivars();
        let main = &state.main;
        let sidebar = &state.sidebar;
        let frame = main.frame();
        let panel_height = frame.size.height as f32 - PANEL_MARGIN * 2.;
        let strip = sidebar_height(panel_height, state.rows.get());
        if !state.enabled.get() || strip == 0. || !main.isVisible() || main.isMiniaturized() {
            main.removeChildWindow(sidebar);
            sidebar.orderOut(None);
            return;
        }
        // The surface ends exactly at the panel edge so the tags meet the panel's
        // stroke; the rest is the transparent margin the tag shadows use.
        let (width, height) = window_size(strip);
        let (width, height) = (width as f64, height as f64);
        let strip_bottom =
            frame.origin.y + frame.size.height - PANEL_MARGIN as f64 - TOP as f64 - strip as f64;
        let target = NSRect::new(
            NSPoint::new(
                frame.origin.x + PANEL_MARGIN as f64 - width,
                strip_bottom - SHADOW as f64,
            ),
            NSSize::new(width, height),
        );
        if sidebar.frame() != target {
            sidebar.setFrame_display(target, true);
        }
        sidebar.setLevel(main.level());
        if sidebar.parentWindow().as_deref() != Some(&**main) {
            unsafe { main.addChildWindow_ordered(sidebar, NSWindowOrderingMode::Above) };
        }
        sidebar.orderFront(None);
    }
}

impl Attachment {
    pub fn install(main: isize, sidebar: isize) -> Result<Rc<Self>, &'static str> {
        let mtm = MainThreadMarker::new().ok_or("tag windows require the main thread")?;
        // SAFETY: these are live NSWindow pointers obtained from GPUI handles.
        let (main, sidebar) = unsafe {
            (
                Retained::retain(main as *mut NSWindow).ok_or("missing main window")?,
                Retained::retain(sidebar as *mut NSWindow).ok_or("missing tag window")?,
            )
        };
        crate::platform::frame_pacing::configure(&sidebar);
        sidebar.setOpaque(false);
        sidebar.setBackgroundColor(Some(&NSColor::clearColor()));
        sidebar.setHasShadow(false);
        unsafe {
            // GPUI creates WindowKind::PopUp as an NSPanel. A borderless,
            // nonactivating panel receives mouse input without taking search's
            // key-window focus. Its content view doesn't request panel focus.
            let _: () = msg_send![&*sidebar, setStyleMask: (1usize << 7)];
            let _: () = msg_send![&*sidebar, setBecomesKeyOnlyIfNeeded: true];
            let _: () = msg_send![&*sidebar, setHidesOnDeactivate: false];
            AnyObject::set_class(&sidebar, nonactivating_class(&sidebar));
        }
        let observer = TagWindowObserver::alloc(mtm).set_ivars(AttachmentState {
            main,
            sidebar,
            enabled: Cell::new(false),
            rows: Cell::new(0),
            pending: Cell::new(false),
        });
        let observer: Retained<TagWindowObserver> = unsafe { msg_send![super(observer), init] };
        let center = NSNotificationCenter::defaultCenter();
        unsafe {
            for name in [
                NSWindowDidResizeNotification,
                NSWindowDidBecomeKeyNotification,
                NSWindowDidDeminiaturizeNotification,
            ] {
                center.addObserver_selector_name_object(
                    &observer,
                    sel!(parentGeometryChanged:),
                    Some(name),
                    Some(&observer.ivars().main),
                );
            }
        }
        Ok(Rc::new(Self { observer }))
    }

    pub fn set_enabled(&self, enabled: bool) {
        if self.observer.ivars().enabled.replace(enabled) != enabled {
            self.observer.schedule();
        }
    }

    pub fn set_rows(&self, rows: usize) {
        if self.observer.ivars().rows.replace(rows) != rows {
            self.observer.schedule();
        }
    }

    pub fn available_height(&self) -> f32 {
        (self.observer.ivars().main.frame().size.height as f32 - PANEL_MARGIN * 2.).max(0.)
    }

    pub fn refresh(&self) {
        self.observer.schedule();
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        unsafe {
            NSNotificationCenter::defaultCenter().removeObserver(&self.observer);
            let _: () = msg_send![objc2::class!(NSObject),
                cancelPreviousPerformRequestsWithTarget: &*self.observer
            ];
        }
        let state = self.observer.ivars();
        state.main.removeChildWindow(&state.sidebar);
        state.sidebar.orderOut(None);
    }
}
