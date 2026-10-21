use objc2_app_kit::NSWindow;
use objc2_quartz_core::CAMetalLayer;

/// GPUI already paces frames with CVDisplayLink. Waiting for another VSync in
/// nextDrawable blocks the main thread and delays mouse events between windows.
pub fn configure(window: &NSWindow) {
    let Some(content) = window.contentView() else {
        return;
    };
    for view in content.subviews() {
        if let Some(layer) = view.layer() {
            if let Some(layer) = layer.downcast_ref::<CAMetalLayer>() {
                layer.setDisplaySyncEnabled(false);
            }
        }
    }
}
