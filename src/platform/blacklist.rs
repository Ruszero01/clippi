//! Blacklist module — detects whether Clippi is the foreground application.
//!
//! Used by FocusService to decide when to auto-hide the window.

#[cfg(target_os = "macos")]
pub fn is_clippi_foreground() -> bool {
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
    if let Some(app) = workspace.frontmostApplication() {
        return app.processIdentifier() == std::process::id() as i32;
    }
    false
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn is_clippi_foreground() -> bool {
    true
}
