//! --- Focus event listener module ---
//! Uses Win32 SetWinEventHook for event-driven focus monitoring
//! Uses NSWorkspace polling for macOS focus monitoring

#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::CloseHandle;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::HWND;
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Accessibility::{
    SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK, WINEVENTPROC,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetAncestor, GetClassNameW, GetForegroundWindow, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, PeekMessageW,
    PostThreadMessageW, TranslateMessage, EVENT_OBJECT_FOCUS, EVENT_SYSTEM_FOREGROUND, GA_ROOT,
    MSG, OBJID_CLIENT, OBJID_WINDOW, PM_REMOVE, WINEVENT_OUTOFCONTEXT, WM_QUIT,
};

#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(target_os = "windows")]
use std::sync::{Mutex, OnceLock};

#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
#[cfg(target_os = "macos")]
use std::sync::{Arc, Mutex, OnceLock};

/// Foreground application information used by the hotkey blacklist UI.
#[derive(Debug, Clone)]
pub struct ForegroundAppInfo {
    pub app_name: String,
    pub window_title: String,
    pub icon_base64: String,
}

/// Last non-Clippi paste target (top-level window).
#[cfg(target_os = "windows")]
static LAST_NON_CLIPPI_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// Our own window handle (set at window creation).
/// Used by `is_clippi_window` to avoid depending on window title.
#[cfg(target_os = "windows")]
static CLIPPI_HWND: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_os = "windows")]
static FOREGROUND_INFO_CACHE: OnceLock<Mutex<Option<CachedWindowsForegroundAppInfo>>> =
    OnceLock::new();

#[cfg(target_os = "windows")]
#[derive(Clone)]
struct CachedWindowsForegroundAppInfo {
    pid: u32,
    app_name: String,
    icon_base64: String,
}

/// Last non-Clippi foreground PID (paste target)
#[cfg(target_os = "macos")]
static LAST_NON_CLIPPI_PID: AtomicI32 = AtomicI32::new(0);

#[cfg(target_os = "macos")]
static FOREGROUND_INFO_CACHE: OnceLock<Mutex<Option<CachedForegroundAppInfo>>> = OnceLock::new();

#[cfg(target_os = "macos")]
#[derive(Clone)]
struct CachedForegroundAppInfo {
    pid: i32,
    info: ForegroundAppInfo,
}

/// FocusWatcher handle
pub struct FocusWatcher {
    #[cfg(target_os = "windows")]
    hook: HWINEVENTHOOK,
    #[cfg(target_os = "windows")]
    object_hook: HWINEVENTHOOK,
    #[cfg(target_os = "windows")]
    thread: Option<std::thread::JoinHandle<()>>,
    #[cfg(target_os = "windows")]
    thread_id: u32,
    #[cfg(target_os = "macos")]
    running: Arc<AtomicBool>,
    #[cfg(target_os = "macos")]
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FocusWatcher {
    #[cfg(target_os = "windows")]
    pub fn stop(&mut self) {
        // SAFETY: `UnhookWinEvent` removes a hook previously registered by
        // `SetWinEventHook`. `PostThreadMessageW` posts a WM_QUIT to the
        // message-pump thread identified by `self.thread_id`; the thread
        // was created in `start_focus_watcher` and is guaranteed alive
        // until `join()` returns.
        unsafe { UnhookWinEvent(self.hook) };
        unsafe { UnhookWinEvent(self.object_hook) };
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    #[cfg(target_os = "macos")]
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    pub fn stop(&mut self) {}
}

/// Register one WinEvent callback for a single event id.
///
/// SAFETY: `callback` must have the exact `WINEVENTPROC` signature; the
/// returned hook stays valid until it is removed with `UnhookWinEvent`.
#[cfg(target_os = "windows")]
unsafe fn register_win_event_hook(
    event: u32,
    callback: unsafe extern "system" fn(
        *mut std::ffi::c_void,
        u32,
        *mut std::ffi::c_void,
        i32,
        i32,
        u32,
        u32,
    ),
) -> HWINEVENTHOOK {
    // SAFETY: transmuting a fn ptr to `*const ()` and back is sound on all
    // Windows ABIs (x86/x64/ARM64). The callback signature matches WINEVENTPROC
    // exactly. If `windows` crate bindings for SetWinEventHook become available
    // with a safe wrapper, this should be migrated.
    let proc: WINEVENTPROC = Some(std::mem::transmute::<
        *const (),
        unsafe extern "system" fn(
            *mut std::ffi::c_void,
            u32,
            *mut std::ffi::c_void,
            i32,
            i32,
            u32,
            u32,
        ),
    >(callback as *const ()));
    SetWinEventHook(
        event,
        event,
        std::ptr::null_mut::<std::ffi::c_void>(),
        proc,
        0,
        0,
        WINEVENT_OUTOFCONTEXT,
    )
}

#[cfg(target_os = "windows")]
pub fn start_focus_watcher() -> Result<FocusWatcher, String> {
    // SAFETY: both callbacks have the exact `WINEVENTPROC` signature; the
    // pointer conversion is confined to `register_win_event_hook`.
    let hook = unsafe { register_win_event_hook(EVENT_SYSTEM_FOREGROUND, win_event_proc) };
    if hook.is_null() {
        return Err("SetWinEventHook failed".to_string());
    }

    // Keyboard focus can move without the foreground window changing. Launcher
    // palettes (Listary, Quicker, …) and other non-activating popups take focus
    // this way, leaving the paste target pointing at whatever was foreground
    // before them — the stale target that used to pull focus away from the very
    // window the user was typing in.
    let object_hook = unsafe { register_win_event_hook(EVENT_OBJECT_FOCUS, focus_object_proc) };
    if object_hook.is_null() {
        // SAFETY: `hook` was registered above and has not been removed yet.
        unsafe { UnhookWinEvent(hook) };
        return Err("SetWinEventHook (focus) failed".to_string());
    }

    // Channel to retrieve the actual thread ID from inside the message pump thread
    let (tx, rx) = std::sync::mpsc::sync_channel::<u32>(0);

    let thread = std::thread::spawn(move || {
        // SAFETY: `GetCurrentThreadId` returns a constant thread ID; always safe.
        let tid = unsafe { GetCurrentThreadId() };
        let _ = tx.send(tid); // blocks until receiver reads — ensures tid is available
                              // SAFETY: `zeroed()` on a POD struct produces a valid zero-valued MSG.
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        loop {
            // SAFETY: `PeekMessageW` with a stack-allocated MSG and null HWND
            // retrieves any message for this thread; the output pointer is valid.
            let ret = unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) };
            if ret == 0 {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            if msg.message == WM_QUIT {
                break;
            }
            // SAFETY: `TranslateMessage` and `DispatchMessageW` are standard
            // message-pump calls on a well-formed MSG obtained from `PeekMessageW`.
            unsafe { TranslateMessage(&msg) };
            unsafe { DispatchMessageW(&msg) };
        }
    });

    let thread_id = rx.recv().unwrap_or(0);

    Ok(FocusWatcher {
        hook,
        object_hook,
        thread: Some(thread),
        thread_id,
    })
}

#[cfg(target_os = "macos")]
pub fn start_focus_watcher() -> Result<FocusWatcher, String> {
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();
    let my_pid = std::process::id() as i32;

    let thread = std::thread::spawn(move || {
        while running_clone.load(Ordering::SeqCst) {
            // NSWorkspace/frontmostApplication can create autoreleased
            // Objective-C temporaries on this non-AppKit thread; drain them
            // every cycle so long idle periods don't accumulate them.
            objc2::rc::autoreleasepool(|_| {
                let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
                if let Some(app) = workspace.frontmostApplication() {
                    let pid = app.processIdentifier();

                    if pid == my_pid {
                        // --- LAST_NON_CLIPPI_PID already holds the correct paste target. ---
                    } else {
                        LAST_NON_CLIPPI_PID.store(pid, Ordering::SeqCst);
                    }
                }
            });

            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    });

    Ok(FocusWatcher {
        running,
        thread: Some(thread),
    })
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn start_focus_watcher() -> Result<FocusWatcher, String> {
    Ok(FocusWatcher {})
}

/// Get the paste target window handle
#[cfg(target_os = "windows")]
pub fn get_last_non_clippi_window() -> Option<HWND> {
    let ptr = LAST_NON_CLIPPI_WINDOW.load(Ordering::SeqCst);
    if ptr == 0 {
        None
    } else {
        Some(ptr as HWND)
    }
}

/// Register our window HWND so the focus watcher can identify us.
#[cfg(target_os = "windows")]
pub fn set_clippi_hwnd(hwnd: isize) {
    CLIPPI_HWND.store(hwnd as usize, Ordering::SeqCst);
}

/// Get the paste target PID
#[cfg(target_os = "macos")]
pub fn get_last_non_clippi_pid() -> Option<i32> {
    let pid = LAST_NON_CLIPPI_PID.load(Ordering::SeqCst);
    if pid == 0 {
        None
    } else {
        Some(pid)
    }
}

#[cfg(target_os = "windows")]
fn is_clippi_window(hwnd: HWND) -> bool {
    let our_hwnd = CLIPPI_HWND.load(Ordering::SeqCst);
    our_hwnd != 0 && hwnd as usize == our_hwnd
}

/// Check if an HWND belongs to our own process.
/// This catches popup menus, dialogs, and other windows created by Clippi
/// that have a different HWND than the main window (so `is_clippi_window`
/// alone would miss them).
#[cfg(target_os = "windows")]
fn is_own_process_window(hwnd: HWND) -> bool {
    let mut pid: u32 = 0;
    // SAFETY: GetWindowThreadProcessId is a read-only query. Returns 0 on failure.
    let tid = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    tid != 0 && pid == std::process::id()
}

/// Whether `hwnd` is a window of Clippi's own process.
///
/// The quick popup asks before borrowing the keyboard focus (`quick_focus`): a
/// focus taken while Clippi itself is in front would be taken from Clippi, with
/// no foreign input queue to join, and the loan would collapse the moment the
/// foreground settles back onto the application — armed for one frame, grey the
/// next.
#[cfg(target_os = "windows")]
pub fn is_own_window(hwnd: HWND) -> bool {
    is_own_process_window(hwnd)
}

/// Resolve the top-level window that owns `hwnd`.
///
/// Win32 activates — and `SetForegroundWindow` accepts — only top-level
/// windows, so every recorded target is normalised through here.
#[cfg(target_os = "windows")]
fn root_window(hwnd: HWND) -> HWND {
    if hwnd.is_null() {
        return hwnd;
    }
    // SAFETY: `GetAncestor(GA_ROOT)` is a read-only query that returns the
    // window itself when it is already top-level.
    unsafe { GetAncestor(hwnd, GA_ROOT) }
}

/// Whether `hwnd` may legitimately receive a simulated paste.
///
/// Rejects Clippi's own windows plus destroyed, hidden, and minimised windows.
/// Activating any of those either does nothing useful or restores a window the
/// user never aimed at — which is what used to yank the foreground away from a
/// launcher's search box and dismiss it.
#[cfg(target_os = "windows")]
pub fn is_valid_paste_target(hwnd: HWND) -> bool {
    if hwnd.is_null() {
        return false;
    }
    // SAFETY: all three calls are read-only queries on a HWND value.
    unsafe {
        if IsWindow(hwnd) == 0 || IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return false;
        }
    }
    !is_clippi_window(hwnd) && !is_own_process_window(hwnd)
}

/// Whether `hwnd` could be restored as a paste target.
///
/// Looser than [`is_valid_paste_target`]: a recorded target may legitimately be
/// minimised or hidden (the user minimised the application it belongs to), and
/// the restore path is able to un-minimise it.
#[cfg(target_os = "windows")]
pub fn is_restorable_paste_target(hwnd: HWND) -> bool {
    if hwnd.is_null() {
        return false;
    }
    // SAFETY: `IsWindow` is a read-only validity query.
    unsafe {
        if IsWindow(hwnd) == 0 {
            return false;
        }
    }
    !is_clippi_window(hwnd) && !is_own_process_window(hwnd)
}

/// Read a window's title.
///
/// `GetWindowTextW` never sends `WM_GETTEXT` across process boundaries, so this
/// is safe to call on foreign windows.
#[cfg(target_os = "windows")]
fn window_title_of(hwnd: HWND) -> String {
    if hwnd.is_null() {
        return String::new();
    }
    let mut buf = [0u16; 512];
    // SAFETY: `buf` is a stack buffer of exactly the length passed along.
    let len = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

/// Human-readable window identity for diagnostics.
#[cfg(target_os = "windows")]
pub fn describe_window(hwnd: HWND) -> String {
    if hwnd.is_null() {
        return "none".to_string();
    }
    // SAFETY: read-only queries; buffers are sized and zeroed here.
    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let mut class_buf = [0u16; 128];
        let class_len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32);
        let class = if class_len > 0 {
            String::from_utf16_lossy(&class_buf[..class_len as usize])
        } else {
            String::from("?")
        };
        format!(
            "0x{:X} (pid={} class=\"{}\" title=\"{}\" visible={} iconic={})",
            hwnd as usize,
            pid,
            class,
            window_title_of(hwnd),
            IsWindowVisible(hwnd) != 0,
            IsIconic(hwnd) != 0,
        )
    }
}

/// Focus and active window reported by one GUI thread.
///
/// `GetGUIThreadInfo` reports the focus of the *queue* the thread is attached
/// to, which is how launcher palettes (Listary, Quicker, …) hold the caret
/// while the window behind them stays in front.
#[cfg(target_os = "windows")]
fn thread_input_windows(thread_id: u32) -> (HWND, HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetGUIThreadInfo, GUITHREADINFO};

    if thread_id == 0 {
        return (std::ptr::null_mut(), std::ptr::null_mut());
    }
    // SAFETY: `GetGUIThreadInfo` is a read-only query; `GUITHREADINFO` is
    // zero-initialised with `cbSize` set as the API requires.
    unsafe {
        let mut info: GUITHREADINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
        if GetGUIThreadInfo(thread_id, &mut info) == 0 {
            return (std::ptr::null_mut(), std::ptr::null_mut());
        }
        (info.hwndFocus, info.hwndActive)
    }
}

/// Thread that owns the foreground window, or `0` when there is none.
#[cfg(target_os = "windows")]
fn foreground_thread_id() -> u32 {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return 0;
    }
    // SAFETY: `GetWindowThreadProcessId` is a read-only query.
    unsafe { GetWindowThreadProcessId(foreground, std::ptr::null_mut()) }
}

/// Focused child window reported by the thread that owns `hwnd`.
///
/// Used to put focus back on the exact control the user was editing when the
/// paste target is re-activated.
#[cfg(target_os = "windows")]
pub fn focused_child_of(hwnd: HWND) -> Option<HWND> {
    if hwnd.is_null() {
        return None;
    }
    // SAFETY: `GetWindowThreadProcessId` is a read-only query.
    let thread_id = unsafe { GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) };
    let (focus, _) = thread_input_windows(thread_id);
    (!focus.is_null()).then_some(focus)
}

/// Window that owns the keyboard focus right now, or `None`.
///
/// This is the window a simulated keystroke actually reaches, so it — not the
/// foreground window — decides where a paste lands when a launcher palette sits
/// in front of the application it was opened from. Falls back to the thread's
/// active window and then to the foreground window when no focus is reported.
#[cfg(target_os = "windows")]
pub fn keyboard_focus_window() -> Option<HWND> {
    let (focus, active) = thread_input_windows(foreground_thread_id());
    if !focus.is_null() {
        return Some(focus);
    }
    if !active.is_null() {
        return Some(active);
    }
    let foreground = unsafe { GetForegroundWindow() };
    (!foreground.is_null()).then_some(foreground)
}

/// Whether `a` and `b` belong to the same top-level window.
#[cfg(target_os = "windows")]
fn same_top_level(a: HWND, b: HWND) -> bool {
    !a.is_null() && !b.is_null() && root_window(a) == root_window(b)
}

/// Record `hwnd` (normalised to its top-level window) as the paste target.
#[cfg(target_os = "windows")]
fn store_paste_target(hwnd: HWND) {
    let root = root_window(hwnd);
    if !is_valid_paste_target(root) {
        return;
    }
    LAST_NON_CLIPPI_WINDOW.store(root as usize, Ordering::SeqCst);
}

/// Sample the window that owns the user's input into the paste target.
///
/// Called right before the quick popup is shown so the target reflects the
/// application the user was working in when they pressed the hotkey, even
/// before any focus event has been observed. The foreground window is sampled
/// first and the keyboard focus second, so a launcher palette that holds the
/// caret without owning the foreground wins. Candidates that belong to Clippi
/// are not targets, so the previous value is kept.
#[cfg(target_os = "windows")]
pub fn sample_paste_target() {
    store_paste_target(unsafe { GetForegroundWindow() });
    if let Some(focus) = keyboard_focus_window() {
        store_paste_target(focus);
    }
}

/// The current foreground window when it can receive a paste.
#[cfg(target_os = "windows")]
pub fn current_paste_target() -> Option<HWND> {
    let fg = unsafe { GetForegroundWindow() };
    is_valid_paste_target(fg).then_some(fg)
}

/// The foreign window that currently owns the user's input, if any.
///
/// The keyboard focus decides: a keystroke follows the focus, so that window is
/// where a paste has to land. It is returned as it is — a launcher's search box
/// is frequently a child of a window that is never shown, and palettes hold the
/// caret without ever becoming the foreground window, so window-style checks
/// (`IsWindowVisible`, `IsIconic`) would throw away exactly the window the user
/// is typing in. Holding the focus is the proof that the window takes input.
/// Clippi's own windows are never reported, so a target reachable only through
/// the focus is left untouched by the restore path. While the quick popup holds
/// the focus on loan, the window the loan came from stands in for it — that is
/// where the focus goes back when the popup is dismissed.
#[cfg(target_os = "windows")]
pub fn live_input_owner() -> Option<HWND> {
    if let Some(hwnd) = keyboard_focus_window().filter(|&hwnd| is_foreign_input_window(hwnd)) {
        return Some(hwnd);
    }
    // The quick popup can hold the keyboard focus on loan while it is on screen
    // (`quick_focus`). That focus is given back the moment the popup hides, so
    // the window it was borrowed from — and not a record of the last foreground
    // application — is where the input returns and where a paste lands.
    if let Some(borrowed) = crate::platform::quick_focus::borrowed_focus_owner() {
        let borrowed = borrowed as HWND;
        if is_foreign_input_window(borrowed) {
            return Some(borrowed);
        }
    }
    current_paste_target()
}

/// Whether `hwnd` belongs to another application and still exists.
///
/// The focused-window counterpart of [`is_valid_paste_target`], without the
/// visibility requirements: a window that owns the keyboard focus receives
/// input whether or not it (or the window it is nested in) is visible.
#[cfg(target_os = "windows")]
fn is_foreign_input_window(hwnd: HWND) -> bool {
    if hwnd.is_null() {
        return false;
    }
    // SAFETY: `IsWindow` is a read-only validity query.
    if unsafe { IsWindow(hwnd) } == 0 {
        return false;
    }
    !is_clippi_window(hwnd) && !is_own_process_window(hwnd)
}

/// Whether the window that owns the input sits outside the window in front.
///
/// That is the signature of a launcher palette: a non-activating window takes
/// the keyboard without ever coming to the front (Quicker and the like), so the
/// caret sits in a window the user is not "in" — the foreground underneath stays
/// where it was. Such a palette keeps its caret only as long as nothing takes
/// that focus from it, which is why the automatic search-box arm asks this
/// first and leaves the palette alone.
#[cfg(target_os = "windows")]
pub fn input_detached_from_foreground() -> bool {
    let Some(focus) = keyboard_focus_window() else {
        return false;
    };
    // SAFETY: read-only query on a handle that is only observed.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return false;
    }
    !same_top_level(focus, foreground)
}

/// Whether a simulated keystroke sent right now would land in `target`.
///
/// A keystroke follows the keyboard focus, so that is what decides; the
/// foreground window is only consulted when no window reports a focus.
#[cfg(target_os = "windows")]
pub fn paste_would_reach(target: HWND) -> bool {
    if target.is_null() {
        return false;
    }
    match keyboard_focus_window() {
        Some(focus) => same_top_level(focus, target),
        None => same_top_level(unsafe { GetForegroundWindow() }, target),
    }
}

/// Whether a simulated keystroke sent right now would land inside one of
/// Clippi's own windows.
#[cfg(target_os = "windows")]
pub fn paste_would_hit_clippi() -> bool {
    match keyboard_focus_window() {
        Some(focus) => is_own_process_window(focus),
        None => {
            let foreground = unsafe { GetForegroundWindow() };
            !foreground.is_null() && is_own_process_window(foreground)
        }
    }
}

/// One-line description of where a simulated keystroke would land right now.
#[cfg(target_os = "windows")]
pub fn describe_input_context(target: Option<HWND>) -> String {
    let focus = keyboard_focus_window();
    let foreground = unsafe { GetForegroundWindow() };
    format!(
        "target={} focus={} foreground={}",
        target.map_or_else(|| "none".to_string(), describe_window),
        focus.map_or_else(|| "none".to_string(), describe_window),
        describe_window(foreground),
    )
}

/// Paste target for the current moment: the window that owns the input when it
/// can receive a paste, otherwise the most recently recorded target.
///
/// The live input owner is preferred because the quick popup never activates —
/// whatever holds the caret, including a launcher palette that is not the
/// foreground window, is where the paste has to land. The recorded target is the
/// fallback for the main window's paste flow, where Clippi itself holds the
/// input.
#[cfg(target_os = "windows")]
pub fn resolve_paste_target() -> Option<HWND> {
    live_input_owner()
        .or_else(|| get_last_non_clippi_window().filter(|&hwnd| is_valid_paste_target(hwnd)))
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread_id: u32,
    _timestamp: u32,
) {
    // Use the HWND carried by the event. A `WINEVENT_OUTOFCONTEXT` callback runs
    // after the fact on our message-pump thread, so re-reading
    // `GetForegroundWindow()` would record whatever window happens to be
    // foreground by then and could miss short-lived activations entirely.
    let candidate = if hwnd.is_null() {
        GetForegroundWindow()
    } else {
        hwnd
    };
    store_paste_target(candidate);
}

/// `EVENT_OBJECT_FOCUS` reports every keyboard-focus change, including the ones
/// that never change the foreground window (launcher palettes, non-activating
/// popups, and child controls).
#[cfg(target_os = "windows")]
unsafe extern "system" fn focus_object_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _thread_id: u32,
    _timestamp: u32,
) {
    // Only window / client focus matters; skip the menu, caret, and
    // value-change objects that share this event id.
    if hwnd.is_null() || (id_object != OBJID_CLIENT && id_object != OBJID_WINDOW) {
        return;
    }
    store_paste_target(hwnd);
}

// --- ── Foreground app info extraction ── ---

/// Get information about the current foreground application.
/// Returns None on unsupported platforms or if the info is unavailable.
pub fn get_foreground_app_info() -> Option<ForegroundAppInfo> {
    #[cfg(target_os = "windows")]
    {
        windows_foreground_info()
    }
    #[cfg(target_os = "macos")]
    {
        macos_foreground_info()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "windows")]
fn windows_foreground_info() -> Option<ForegroundAppInfo> {
    // SAFETY: `GetForegroundWindow`, `GetWindowThreadProcessId`, `OpenProcess`
    // (PROCESS_QUERY_LIMITED_INFORMATION), `QueryFullProcessImageNameW`, and
    // `SHGetFileInfoW` are all read-only or constrained queries. Stack-allocated
    // buffers (exe_buf, SHFILEINFOW) are properly sized and zeroed. `CloseHandle`
    // is always called on non-null handles. Icon extraction delegates to
    // `hicon_to_base64_png` which takes HICON ownership.
    unsafe {
        // --- Report the window a paste would actually land in: the live
        // --- foreground when it can receive input, otherwise the recorded
        // --- target. The focus watcher keeps that target current even for
        // --- palettes that take keyboard focus without becoming foreground. ---
        let hwnd = resolve_paste_target()?;

        let window_title = window_title_of(hwnd);

        // --- Get PID ---
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 || pid == std::process::id() {
            // pid == 0: window is invalid / closed
            // pid == self: window belongs to Clippi (popup menu, dialog, etc.)
            // In either case, return None so the UI shows the last known non-Clippi app.
            return None;
        }

        let cache = FOREGROUND_INFO_CACHE.get_or_init(|| Mutex::new(None));
        if let Ok(cache_guard) = cache.lock() {
            if let Some(cached) = cache_guard.as_ref().filter(|cached| cached.pid == pid) {
                return Some(ForegroundAppInfo {
                    app_name: cached.app_name.clone(),
                    window_title,
                    icon_base64: cached.icon_base64.clone(),
                });
            }
        }

        // --- Get exe path ---
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return Some(ForegroundAppInfo {
                app_name: String::new(),
                window_title,
                icon_base64: String::new(),
            });
        }
        let mut exe_buf = [0u16; 260];
        let mut exe_len = exe_buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            0, // PROCESS_NAME_WIN32
            exe_buf.as_mut_ptr(),
            &mut exe_len,
        );
        CloseHandle(process);
        if result == 0 {
            return Some(ForegroundAppInfo {
                app_name: String::new(),
                window_title,
                icon_base64: String::new(),
            });
        }

        let exe_path = String::from_utf16_lossy(&exe_buf[..exe_len as usize]);
        let app_name = std::path::Path::new(&exe_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| {
                let mut chars = s.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .unwrap_or_default();

        // --- Extract icon ---
        let wide_path: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut shfi: SHFILEINFOW = std::mem::zeroed();
        let icon_result = SHGetFileInfoW(
            wide_path.as_ptr(),
            0,
            &mut shfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        // --- hicon_to_base64_png always takes ownership and calls DestroyIcon internally ---
        let icon_base64 = if icon_result != 0 && !shfi.hIcon.is_null() {
            super::util::hicon_to_base64_png(shfi.hIcon, 32).unwrap_or_default()
        } else {
            String::new()
        };

        if let Ok(mut cache_guard) = cache.lock() {
            *cache_guard = Some(CachedWindowsForegroundAppInfo {
                pid,
                app_name: app_name.clone(),
                icon_base64: icon_base64.clone(),
            });
        }

        Some(ForegroundAppInfo {
            app_name,
            window_title,
            icon_base64,
        })
    }
}

#[cfg(target_os = "macos")]
fn macos_foreground_info() -> Option<ForegroundAppInfo> {
    // This runs every polling cycle, so drain autoreleased
    // NSWorkspace/NSRunningApplication/NSImage temporaries per call.
    // Only owned Rust values (PID, String) escape the pool.
    objc2::rc::autoreleasepool(|_| {
        let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
        let app = workspace.frontmostApplication()?;
        let pid = app.processIdentifier();

        if pid == std::process::id() as i32 {
            // --- Clippi itself — no foreground info to show ---
            return None;
        }

        let cache = FOREGROUND_INFO_CACHE.get_or_init(|| Mutex::new(None));
        if let Ok(cache_guard) = cache.lock() {
            if let Some(cached) = cache_guard.as_ref().filter(|cached| cached.pid == pid) {
                return Some(cached.info.clone());
            }
        }

        // --- Use generated methods (nil-safe via Option) ---
        let app_name = app
            .localizedName()
            .map(|n| n.to_string())
            .unwrap_or_default();
        let icon_base64 = app
            .icon()
            .and_then(|i| super::util::nsimage_to_base64_png(&i, 32))
            .unwrap_or_default();

        Some(ForegroundAppInfo {
            app_name,
            window_title: String::new(), // macOS window title extraction requires extra permissions
            icon_base64,
        })
        .inspect(|info| {
            if let Ok(mut cache_guard) = cache.lock() {
                *cache_guard = Some(CachedForegroundAppInfo {
                    pid,
                    info: info.clone(),
                });
            }
        })
    })
}
