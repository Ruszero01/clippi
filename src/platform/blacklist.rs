//! Blacklist module — detects whether Clippi is the foreground application.
//!
//! --- Used by FocusService to decide when to auto-hide the window. ---
//! --- On Windows, `WindowManager::is_self_foreground` uses direct HWND ---
//! --- comparison instead of this function. ---

#[cfg(target_os = "macos")]
pub fn is_clippi_foreground() -> bool {
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
    if let Some(app) = workspace.frontmostApplication() {
        return app.processIdentifier() == std::process::id() as i32;
    }
    false
}

/// Windows uses `WindowManager::is_self_foreground` with HWND comparison;
/// this stub exists for API completeness and is only used on non-Windows platforms.
#[cfg(target_os = "windows")]
#[allow(dead_code)]
pub fn is_clippi_foreground() -> bool {
    false
}

/// Linux / other: not yet supported — always report Clippi in foreground
/// to prevent incorrect auto-hide behavior.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn is_clippi_foreground() -> bool {
    true
}
