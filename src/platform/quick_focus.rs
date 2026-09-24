//! Keyboard focus loan and input-method sink for the quick paste popup.
//!
//! The quick popup is a `WS_EX_NOACTIVATE` window: it is shown without taking
//! the foreground, so the application the user was working in stays in front and
//! keeps its place in the z-order. That style also means the popup never owns
//! the keyboard focus, which is why its search text is read from a low-level
//! keyboard hook (`platform::keyboard_hook`) instead of from a text field.
//!
//! A hook can translate letter keys, but it can never run an input method. The
//! moment the user searches in Chinese (or any other composed script) the
//! composition has to happen in a window that owns the keyboard focus, because
//! that is where Windows delivers `WM_IME_*`. Clicking the search box therefore
//! *borrows* the focus: this module joins the input queue of the foreground
//! thread and moves the focus of that queue to the popup. The foreground window
//! itself never changes, so the window underneath is neither raised nor
//! deactivated — which is what keeps launcher palettes such as Quicker's search
//! box alive while Clippi is on screen.
//!
//! While the popup owns the focus the keyboard hook stops translating text keys
//! and lets them through; they reach the popup, where the input method turns
//! them into a composition string and then into committed text. A window
//! subclass picks up those messages, and both the composition and the committed
//! text are parked in a shared buffer that the window manager's poll drains into
//! `QuickPasteView` — the same hand-off the keyboard hook uses for its keys.
//!
//! Releasing hands the focus back to the window that had it, and the popup goes
//! back to being driven by the hook alone.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// Text the input method produced for the popup.
#[derive(Default, Clone)]
struct ImeText {
    /// Composition being typed (pinyin, zhuyin, kana, …), shown inline after
    /// the query. Empty while the input method is not composing.
    composition: String,
    /// Text the input method already committed; drained by the poll loop.
    committed: String,
    /// High half of a surrogate pair that arrived in two messages.
    pending_surrogate: Option<u16>,
    /// Set whenever `composition` or `committed` changed.
    dirty: bool,
}

static IME_TEXT: OnceLock<Mutex<ImeText>> = OnceLock::new();

fn ime_text() -> &'static Mutex<ImeText> {
    IME_TEXT.get_or_init(|| Mutex::new(ImeText::default()))
}

/// An input method update observed since the last drain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImeUpdate {
    /// Composition currently being typed; empty when there is none.
    pub composition: String,
    /// Text committed since the last drain.
    pub committed: String,
}

/// Drain the input method text observed since the last call.
///
/// Returns `None` while nothing changed, so the poll loop stays a no-op outside
/// of an active composition.
pub fn take_ime_update() -> Option<ImeUpdate> {
    let Ok(mut text) = ime_text().lock() else {
        return None;
    };
    if !text.dirty {
        return None;
    }
    text.dirty = false;
    Some(ImeUpdate {
        composition: text.composition.clone(),
        committed: std::mem::take(&mut text.committed),
    })
}

/// Drop every pending input method text (popup dismissed).
pub fn reset_ime_text() {
    let Ok(mut text) = ime_text().lock() else {
        return;
    };
    text.composition.clear();
    text.committed.clear();
    text.pending_surrogate = None;
    text.dirty = false;
}

/// Append one UTF-16 code unit delivered by the input method.
///
/// Only ever called on the window thread, so a character outside the basic
/// plane — which arrives as a surrogate pair in two messages — is reassembled
/// here instead of being dropped half-way.
fn push_ime_unit(text: &mut ImeText, unit: u16) {
    match text.pending_surrogate.take() {
        Some(high) => {
            if let Ok(decoded) = char::decode_utf16([high, unit]).collect::<Result<String, _>>() {
                text.committed.push_str(&decoded);
                text.dirty = true;
            }
        }
        None if (0xD800..0xDC00).contains(&unit) => text.pending_surrogate = Some(unit),
        None => {
            if let Some(character) = char::from_u32(unit as u32) {
                text.committed.push(character);
                text.dirty = true;
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::*;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::Input::Ime::{
        ImmGetCompositionStringW, ImmGetContext, ImmGetOpenStatus, ImmReleaseContext,
        ImmSetCandidateWindow, ImmSetCompositionWindow, ImmSetOpenStatus, CANDIDATEFORM,
        CFS_CANDIDATEPOS, CFS_POINT, COMPOSITIONFORM, GCS_COMPSTR, GCS_RESULTSTR, HIMC,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        ActivateKeyboardLayout, GetFocus, GetKeyboardLayout, SetFocus, HKL,
    };
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, IsWindow, GUITHREADINFO,
        WM_CHAR, WM_IME_CHAR, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION,
        WM_NCDESTROY,
    };

    /// Subclass cookie; only has to be unique within our own window.
    const SUBCLASS_ID: usize = 0x434C_5050;

    /// The quick popup, or null before it has been created.
    static POPUP: AtomicUsize = AtomicUsize::new(0);
    /// Whether the popup currently holds the borrowed keyboard focus.
    static HELD: AtomicBool = AtomicBool::new(false);
    /// Window that owned the focus before the borrow, restored on release.
    static PREVIOUS_FOCUS: AtomicUsize = AtomicUsize::new(0);
    /// Input queue we joined, or 0 when no attach was needed.
    static ATTACHED_THREAD: AtomicU32 = AtomicU32::new(0);
    /// Keyboard layout this thread used before the borrow, restored on release.
    static PREVIOUS_LAYOUT: AtomicUsize = AtomicUsize::new(0);
    /// Whether the message subclass is installed on the popup.
    static SUBCLASSED: AtomicBool = AtomicBool::new(false);
    /// Caret position (logical client pixels) the input method windows follow.
    static CARET_X: AtomicU32 = AtomicU32::new(0);
    static CARET_Y: AtomicU32 = AtomicU32::new(0);

    pub fn install(hwnd: isize) {
        let hwnd = hwnd as HWND;
        if hwnd.is_null() || SUBCLASSED.load(Ordering::SeqCst) {
            return;
        }
        POPUP.store(hwnd as usize, Ordering::SeqCst);
        // SAFETY: `hwnd` is Clippi's own popup window, created on this thread,
        // and `quick_subclass_proc` is a valid `SUBCLASSPROC` that stays alive
        // for as long as the window is subclassed.
        let installed =
            unsafe { SetWindowSubclass(hwnd, Some(quick_subclass_proc), SUBCLASS_ID, 0) };
        if installed == 0 {
            log::warn!(
                "quick popup input method subclass unavailable; the search box stays \
                 limited to characters the keyboard hook can translate"
            );
            return;
        }
        SUBCLASSED.store(true, Ordering::SeqCst);
    }

    pub fn uninstall() {
        let hwnd = POPUP.swap(0, Ordering::SeqCst) as HWND;
        if hwnd.is_null() || !SUBCLASSED.swap(false, Ordering::SeqCst) {
            return;
        }
        // SAFETY: the subclass was installed on this thread and the window is
        // still alive; removing it restores the original window procedure.
        unsafe { RemoveWindowSubclass(hwnd, Some(quick_subclass_proc), SUBCLASS_ID) };
    }

    /// Move the caret the input method windows follow, in logical client pixels.
    pub fn set_caret_point(x: f32, y: f32) {
        CARET_X.store(x.max(0.0) as u32, Ordering::SeqCst);
        CARET_Y.store(y.max(0.0) as u32, Ordering::SeqCst);
    }

    pub fn acquire() -> bool {
        let popup = POPUP.load(Ordering::SeqCst) as HWND;
        if popup.is_null() {
            return false;
        }
        if HELD.load(Ordering::SeqCst) {
            return true;
        }
        // SAFETY: read-only queries plus an input queue attachment that
        // `release`/`abandon` always undo. Only the focus of the joined queue
        // moves — the foreground window is never changed.
        unsafe {
            let current = GetCurrentThreadId();
            let foreground = GetForegroundWindow();
            // Borrowing only means something from another application. While one
            // of our own windows is in front — most of all the main window being
            // hidden the moment before the popup is shown — the focus would be
            // taken from Clippi, the two input queues would never be joined, and
            // the loan would collapse as soon as the foreground settles back onto
            // the application: armed for one frame, grey the next. Refusing here
            // lets the caller keep trying until the application is in front
            // again, which is the borrow it actually wants.
            if foreground.is_null() || crate::platform::focus::is_own_window(foreground) {
                return false;
            }
            let owner = GetWindowThreadProcessId(foreground, std::ptr::null_mut());
            let (focus, active) = thread_input_windows(owner);
            let previous = [focus, active, foreground]
                .into_iter()
                .find(|&window| !window.is_null() && window != popup);
            PREVIOUS_FOCUS.store(
                previous.map_or(0, |window| window as usize),
                Ordering::SeqCst,
            );

            // Take over the input context the user was typing in before the
            // focus moves. The keyboard layout lives on the thread, so without
            // this the search box would translate keys with Clippi's layout;
            // the input method state lives on the window, so an input method
            // that was open would come up closed and a user typing Chinese
            // would get latin letters instead of a composition.
            adopt_input_context(owner, previous);

            // A palette that never comes to the front holds its caret in a window
            // that is not the foreground one. Taking that focus is what such a
            // panel reads as "the user left", so it dismisses itself — and the
            // paste target goes with it. Worth a line, because the disappearance
            // happens in another process and leaves no other trace here.
            if crate::platform::focus::input_detached_from_foreground() {
                log::info!(
                    "quick search box takes the keyboard focus of a window that is not in \
                     front; a launcher palette holding it will dismiss itself"
                );
            }

            let attached =
                owner != 0 && owner != current && AttachThreadInput(current, owner, 1) != 0;
            ATTACHED_THREAD.store(if attached { owner } else { 0 }, Ordering::SeqCst);

            // `SetFocus` is all it takes: it moves the focus of the joined queue
            // to the popup, so keys — and with them the input method — arrive
            // here. The active window is deliberately left alone: activating the
            // popup would deactivate the window underneath, which is what makes
            // launcher palettes dismiss themselves.
            SetFocus(popup);
            if GetFocus() != popup {
                detach();
                // The input context was taken over for a borrow that never
                // happened; put the thread back the way it was.
                restore_input_context();
                log::warn!(
                    "quick search box could not take the keyboard focus for the input method"
                );
                return false;
            }
            HELD.store(true, Ordering::SeqCst);
            log::info!(
                "quick search box took the keyboard focus for the input method (previous {})",
                describe(previous)
            );
            true
        }
    }

    /// Release the borrow.
    ///
    /// `restore_focus` is false for a borrow that already went stale: the user
    /// moved on to another window, and pulling the focus back to the window that
    /// had it before would fight that.
    pub fn release(restore_focus: bool) -> bool {
        if !HELD.swap(false, Ordering::SeqCst) {
            return false;
        }
        // SAFETY: the focus is handed back to the window that held it, then the
        // input queues are separated again. Both handles belong to windows that
        // were only read until now.
        unsafe {
            let previous = PREVIOUS_FOCUS.swap(0, Ordering::SeqCst) as HWND;
            let restore = restore_focus && !previous.is_null() && IsWindow(previous) != 0;
            if restore {
                SetFocus(previous);
            }
            // Read before the queues separate again: afterwards this thread's
            // focus is its own queue's business and says nothing about where the
            // caret ended up.
            let focus_now = describe(Some(GetFocus()));
            detach();
            restore_input_context();
            let outcome = if restore {
                format!("handed back to {}", describe(Some(previous)))
            } else {
                "dropped".to_string()
            };
            log::info!(
                "quick search box released the keyboard focus ({outcome}, focus now {focus_now})"
            );
        }
        true
    }

    /// Whether the popup currently owns the keyboard focus of its input queue.
    ///
    /// `GetFocus` cannot answer this from the keyboard hook, which pumps on a
    /// thread of its own — that function reports the focus of the *calling*
    /// thread's queue. `GetGUIThreadInfo` answers for the queue that owns the
    /// popup, which is the one the keys are routed to.
    pub fn popup_has_keyboard_focus() -> bool {
        if !HELD.load(Ordering::SeqCst) {
            return false;
        }
        let popup = POPUP.load(Ordering::SeqCst) as HWND;
        if popup.is_null() {
            return false;
        }
        // SAFETY: read-only queries on handles that are either ours or merely
        // observed.
        unsafe {
            // A keystroke follows the focus of the queue that receives it, and
            // that is the foreground thread's queue. The popup is part of it
            // either because the two queues were joined, or because the popup
            // sits on the foreground thread to begin with. Anything else means
            // the keys are routed past the popup, however focused it looks.
            let foreground = GetForegroundWindow();
            let foreground_thread = if foreground.is_null() {
                0
            } else {
                GetWindowThreadProcessId(foreground, std::ptr::null_mut())
            };
            let joined = ATTACHED_THREAD.load(Ordering::SeqCst);
            let shares_input_queue = (joined != 0 && joined == foreground_thread)
                || foreground_thread == GetWindowThreadProcessId(popup, std::ptr::null_mut());
            if !shares_input_queue {
                return false;
            }
            let (focus, _) = thread_input_windows_of_window(popup);
            focus == popup
        }
    }

    /// Input method state of the popup itself: `(open, composing)`.
    ///
    /// Asked from the keyboard hook, so it must not depend on the focus of the
    /// calling thread's queue: the popup's own window is the question.
    pub fn ime_state() -> Option<(bool, bool)> {
        let popup = POPUP.load(Ordering::SeqCst) as HWND;
        if popup.is_null() {
            return None;
        }
        // SAFETY: read-only queries on our own window's input method context,
        // which is released on every path.
        unsafe {
            let context = ImmGetContext(popup);
            if context.is_null() {
                return None;
            }
            let open = ImmGetOpenStatus(context) != 0;
            let composing =
                ImmGetCompositionStringW(context, GCS_COMPSTR, std::ptr::null_mut(), 0) > 0;
            ImmReleaseContext(popup, context);
            Some((open, composing))
        }
    }

    /// The window the focus was borrowed from, while the borrow is held.
    ///
    /// The popup gives the focus back the moment it hides, so this is where the
    /// input returns — and therefore where a keystroke sent after the popup is
    /// dismissed lands.
    pub fn borrowed_focus_owner() -> Option<isize> {
        if !HELD.load(Ordering::SeqCst) {
            return None;
        }
        let previous = PREVIOUS_FOCUS.load(Ordering::SeqCst);
        if previous == 0 {
            return None;
        }
        // SAFETY: read-only validity query on a handle that was only observed.
        if unsafe { IsWindow(previous as HWND) } == 0 {
            return None;
        }
        Some(previous as isize)
    }

    /// Whether the user moved on, i.e. the loan no longer belongs to the pair of
    /// windows it was taken from.
    ///
    /// A changed foreground window is the proof. An application that takes its
    /// focus back inside the same input queue is not the user moving on, and
    /// reading it as such is what used to leave a dismissed popup holding the
    /// caret — with the paste then landing in Clippi instead of the application
    /// the user was typing in. Without a joined queue the popup shares its queue
    /// with the foreground window anyway, so a focus that left the popup is
    /// deliberate and the loan ends with it.
    pub fn lost() -> bool {
        if !HELD.load(Ordering::SeqCst) {
            return false;
        }
        let popup = POPUP.load(Ordering::SeqCst) as HWND;
        if popup.is_null() {
            return true;
        }
        // SAFETY: read-only queries on handles that are either ours or merely
        // observed.
        unsafe {
            let joined = ATTACHED_THREAD.load(Ordering::SeqCst);
            let foreground = GetForegroundWindow();
            // Our own window in front is our own doing, not the user walking
            // away: the popup never activates, so this is the main window being
            // shown or hidden. The focus belongs to the queue the loan came from
            // and can be taken back, so dropping it here would disarm a search
            // box the user is still working in.
            if !foreground.is_null() && crate::platform::focus::is_own_window(foreground) {
                return false;
            }
            let foreground_thread = if foreground.is_null() {
                0
            } else {
                GetWindowThreadProcessId(foreground, std::ptr::null_mut())
            };
            if joined != 0 {
                return foreground_thread != joined;
            }
            let popup_thread = GetWindowThreadProcessId(popup, std::ptr::null_mut());
            if foreground_thread != popup_thread {
                return true;
            }
            GetFocus() != popup
        }
    }

    /// Take the focus back if it slipped away while the user is still working in
    /// the same pair of windows.
    ///
    /// Applications that watch for focus loss — browsers and Electron apps give
    /// the focus back to themselves — can quietly retake it inside the same
    /// input queue, which would leave the search box looking focused while it
    /// receives nothing. The unchanged foreground window is the proof that this
    /// is a slip rather than the user moving on, so the loan is asserted again.
    /// A failed attempt means the popup can no longer receive keys, and the
    /// caller drops the loan.
    pub fn reassert() -> bool {
        if !HELD.load(Ordering::SeqCst) {
            return false;
        }
        let popup = POPUP.load(Ordering::SeqCst) as HWND;
        if popup.is_null() {
            return false;
        }
        // SAFETY: read-only queries plus a focus assertion on our own window,
        // whose input queue is still joined to the one the loan came from.
        unsafe {
            let joined = ATTACHED_THREAD.load(Ordering::SeqCst);
            let foreground = GetForegroundWindow();
            // Clippi itself in front is a transient of our own UI, and the focus
            // is still ours to take back — the same reasoning as `lost`.
            if !foreground.is_null() && crate::platform::focus::is_own_window(foreground) {
                if GetFocus() != popup {
                    SetFocus(popup);
                }
                return GetFocus() == popup;
            }
            if joined == 0 {
                return GetFocus() == popup;
            }
            let foreground_thread = if foreground.is_null() {
                0
            } else {
                GetWindowThreadProcessId(foreground, std::ptr::null_mut())
            };
            if foreground_thread != joined {
                return false;
            }
            if GetFocus() != popup {
                SetFocus(popup);
            }
            GetFocus() == popup
        }
    }

    /// Take over the keyboard layout and input method state of the window the
    /// focus is being borrowed from.
    ///
    /// Both are part of the input context the user was typing in: the layout
    /// decides which keys the input method turns into text, and an input method
    /// that is open has to stay open, or the search box would silently fall back
    /// to latin letters. Only ever opens the input method — closing it would
    /// take away a composition a user asked for, while leaving it open is one
    /// Shift press away from being undone.
    fn adopt_input_context(owner_thread: u32, previous: Option<HWND>) {
        // SAFETY: read-only layout and input method queries, plus one layout
        // activation (undone by `restore_input_context`) and one open-status
        // change on our own window's input method context.
        unsafe {
            let layout = if owner_thread == 0 {
                std::ptr::null_mut()
            } else {
                GetKeyboardLayout(owner_thread)
            };
            if !layout.is_null() {
                let current = GetKeyboardLayout(GetCurrentThreadId());
                if current != layout {
                    PREVIOUS_LAYOUT.store(current as usize, Ordering::SeqCst);
                    ActivateKeyboardLayout(layout, 0);
                }
            }

            let Some(previous) = previous else {
                return;
            };
            // A launcher's search box may belong to another process; a context
            // that cannot be read that way is simply treated as closed.
            let previous_context = ImmGetContext(previous);
            if previous_context.is_null() {
                return;
            }
            let open = ImmGetOpenStatus(previous_context) != 0;
            ImmReleaseContext(previous, previous_context);
            if !open {
                return;
            }
            let popup = POPUP.load(Ordering::SeqCst) as HWND;
            if popup.is_null() {
                return;
            }
            let context = ImmGetContext(popup);
            if context.is_null() {
                return;
            }
            ImmSetOpenStatus(context, 1);
            ImmReleaseContext(popup, context);
        }
    }

    /// Put the thread's keyboard layout back the way the borrow found it.
    fn restore_input_context() {
        let previous = PREVIOUS_LAYOUT.swap(0, Ordering::SeqCst);
        if previous == 0 {
            return;
        }
        // SAFETY: the handle came from `GetKeyboardLayout` for this thread and
        // is only used to put that thread's layout back.
        unsafe { ActivateKeyboardLayout(previous as HKL, 0) };
    }

    fn detach() {
        let joined = ATTACHED_THREAD.swap(0, Ordering::SeqCst);
        if joined == 0 {
            return;
        }
        // SAFETY: `joined` is the thread this module attached to; detaching is
        // the documented way to undo `AttachThreadInput`.
        unsafe { AttachThreadInput(GetCurrentThreadId(), joined, 0) };
    }

    /// Focus and active window of the input queue that owns `hwnd`.
    fn thread_input_windows_of_window(hwnd: HWND) -> (HWND, HWND) {
        // SAFETY: `GetWindowThreadProcessId` is a read-only query that returns 0
        // for an invalid handle, which `thread_input_windows` treats as "no
        // queue".
        let thread = unsafe { GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) };
        thread_input_windows(thread)
    }

    /// Focus and active window of one thread's input queue.
    fn thread_input_windows(thread_id: u32) -> (HWND, HWND) {
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

    fn describe(hwnd: Option<HWND>) -> String {
        hwnd.map_or_else(
            || "none".to_string(),
            crate::platform::focus::describe_window,
        )
    }

    /// Where the input method should draw its composition and candidate windows.
    ///
    /// The coordinates the input method context expects are physical client
    /// pixels, while the popup lays its search field out in logical ones.
    fn caret_point(hwnd: HWND) -> POINT {
        // SAFETY: `GetDpiForWindow` is a read-only query on our own window.
        let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
        POINT {
            x: (CARET_X.load(Ordering::SeqCst) as f32 * scale) as i32,
            y: (CARET_Y.load(Ordering::SeqCst) as f32 * scale) as i32,
        }
    }

    /// Keep the composition and candidate windows next to the search field.
    fn position_ime_windows(hwnd: HWND) {
        let point = caret_point(hwnd);
        // SAFETY: the context is released on every path, and the structs are
        // plain by-value configuration passed by pointer as the API expects.
        unsafe {
            let context = ImmGetContext(hwnd);
            if context.is_null() {
                return;
            }
            let composition = COMPOSITIONFORM {
                dwStyle: CFS_POINT,
                ptCurrentPos: point,
                // Unused for `CFS_POINT`.
                rcArea: std::mem::zeroed::<RECT>(),
            };
            ImmSetCompositionWindow(context, &composition);
            let candidate = CANDIDATEFORM {
                dwIndex: 0,
                dwStyle: CFS_CANDIDATEPOS,
                ptCurrentPos: point,
                // Unused for `CFS_CANDIDATEPOS`.
                rcArea: std::mem::zeroed::<RECT>(),
            };
            ImmSetCandidateWindow(context, &candidate);
            ImmReleaseContext(hwnd, context);
        }
    }

    /// Read one composition string out of the input method context.
    fn composition_string(context: HIMC, kind: u32) -> Option<String> {
        // SAFETY: the length is queried with a null buffer first, and the second
        // call fills a buffer of exactly the reported length.
        unsafe {
            let length = ImmGetCompositionStringW(context, kind, std::ptr::null_mut(), 0);
            if length <= 0 {
                return None;
            }
            let mut buffer = vec![0u16; length as usize / 2];
            let written = ImmGetCompositionStringW(
                context,
                kind,
                buffer.as_mut_ptr() as *mut std::ffi::c_void,
                length as u32,
            );
            if written <= 0 {
                return None;
            }
            let units = (written as usize / 2).min(buffer.len());
            String::from_utf16(&buffer[..units]).ok()
        }
    }

    fn read_composition(hwnd: HWND, lparam: LPARAM) {
        let flags = lparam as u32;
        // SAFETY: the context is released on every path.
        unsafe {
            let context = ImmGetContext(hwnd);
            if context.is_null() {
                return;
            }
            let result = composition_string(context, GCS_RESULTSTR);
            let composition = composition_string(context, GCS_COMPSTR);
            ImmReleaseContext(hwnd, context);

            let Ok(mut text) = ime_text().lock() else {
                return;
            };
            if flags & GCS_RESULTSTR != 0 {
                if let Some(result) = result {
                    if !result.is_empty() {
                        text.committed.push_str(&result);
                        text.dirty = true;
                    }
                }
            }
            // A composition message always describes the current composition,
            // including the empty one that ends it.
            if flags & GCS_COMPSTR != 0 {
                let composition = composition.unwrap_or_default();
                if text.composition != composition {
                    text.composition = composition;
                    text.dirty = true;
                }
            }
        }
    }

    fn begin_composition(hwnd: HWND) {
        position_ime_windows(hwnd);
    }

    fn end_composition() {
        let Ok(mut text) = ime_text().lock() else {
            return;
        };
        if text.composition.is_empty() {
            return;
        }
        text.composition.clear();
        text.dirty = true;
    }

    fn push_committed(unit: u16) {
        // A control character (Tab, the Ctrl+V the popup itself may see, …) is
        // not search text: only the keys the keyboard hook deliberately leaves
        // alone reach the popup this way, and putting any of them in the query
        // would add invisible junk. Surrogate halves are not control characters
        // and still go through, so a character outside the basic plane survives.
        if char::from_u32(unit as u32).is_some_and(char::is_control) {
            return;
        }
        let Ok(mut text) = ime_text().lock() else {
            return;
        };
        push_ime_unit(&mut text, unit);
    }

    /// Window procedure installed on the popup for its lifetime.
    ///
    /// Only the input method traffic is taken over: the composition string and
    /// the text the input method commits. Every other message — including the
    /// keys and mouse events GPUI needs — keeps its normal path, so installing
    /// this cannot disturb the popup's regular message handling.
    ///
    /// SAFETY: Windows calls this with the popup's window handle and a message
    /// payload that matches the message being handled.
    unsafe extern "system" fn quick_subclass_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        match message {
            WM_IME_STARTCOMPOSITION => {
                begin_composition(hwnd);
                0
            }
            WM_IME_COMPOSITION => {
                read_composition(hwnd, lparam);
                0
            }
            WM_IME_ENDCOMPOSITION => {
                end_composition();
                0
            }
            // Text that never went through a composition (direct conversion,
            // full width punctuation, …) arrives as a character message.
            WM_IME_CHAR | WM_CHAR => {
                push_committed(wparam as u16);
                0
            }
            WM_NCDESTROY => {
                SUBCLASSED.store(false, Ordering::SeqCst);
                // SAFETY: same cookie the subclass was installed with.
                RemoveWindowSubclass(hwnd, Some(quick_subclass_proc), SUBCLASS_ID);
                // SAFETY: forwarding the final message to the original procedure.
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
            // SAFETY: forwarding every other message to the original procedure.
            _ => DefSubclassProc(hwnd, message, wparam, lparam),
        }
    }
}

/// Install the input method sink on the quick popup window.
///
/// Must be called from the thread that owns the window (GPUI's main thread).
pub fn install(popup_hwnd: isize) {
    #[cfg(target_os = "windows")]
    windows_impl::install(popup_hwnd);
    #[cfg(not(target_os = "windows"))]
    let _ = popup_hwnd;
}

/// Remove the input method sink (application shutdown).
pub fn uninstall() {
    #[cfg(target_os = "windows")]
    windows_impl::uninstall();
}

/// Tell the input method where the search caret is, in logical client pixels.
pub fn set_caret_point(x: f32, y: f32) {
    #[cfg(target_os = "windows")]
    windows_impl::set_caret_point(x, y);
    #[cfg(not(target_os = "windows"))]
    let _ = (x, y);
}

/// Borrow the keyboard focus for the popup's search box.
///
/// Returns whether the popup now owns the focus, i.e. whether keys and input
/// method messages reach it. While the borrow is held the low-level keyboard
/// hook stops translating text keys, so nothing is swallowed twice.
pub fn acquire_search_focus() -> bool {
    let acquired = {
        #[cfg(target_os = "windows")]
        {
            windows_impl::acquire()
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    };
    if acquired {
        crate::platform::keyboard_hook::set_search_focused(true);
    }
    acquired
}

/// Hand the borrowed keyboard focus back.
///
/// The focus only returns to the window that had it while the popup still owns
/// it; a borrow that already went stale is merely detached.
pub fn release_search_focus() -> bool {
    crate::platform::keyboard_hook::set_search_focused(false);
    #[cfg(target_os = "windows")]
    {
        let restore = !windows_impl::lost();
        windows_impl::release(restore)
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// The window the popup borrowed the keyboard focus from, while it still holds
/// it.
///
/// Dismissing the popup gives the focus back to that window, so it — and never
/// a stale record of the last foreground application — is where a keystroke
/// sent after the popup closes lands.
pub fn borrowed_focus_owner() -> Option<isize> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::borrowed_focus_owner()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// Drop the borrow without touching the focus, for a borrow that went stale.
pub fn abandon_search_focus() -> bool {
    crate::platform::keyboard_hook::set_search_focused(false);
    #[cfg(target_os = "windows")]
    {
        windows_impl::release(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// Whether the popup really receives the keys that follow the keyboard focus.
///
/// A borrow can look healthy — the focus reports the popup — while the keys are
/// routed to another input queue altogether. The keyboard hook asks before
/// handing a keystroke over, so a takeover that did not take effect cannot
/// swallow the search.
pub fn popup_has_keyboard_focus() -> bool {
    #[cfg(target_os = "windows")]
    {
        windows_impl::popup_has_keyboard_focus()
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// Input method state of the popup itself: `(open, composing)`, or `None` while
/// there is no popup to ask.
///
/// While the search box holds the focus this — not whatever the application
/// underneath is doing — decides who owns the text keys.
pub fn search_ime_state() -> Option<(bool, bool)> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::ime_state()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// Take the focus back if it slipped away inside the same pair of windows.
///
/// Returns whether the popup still owns the keyboard focus afterwards.
pub fn reassert_search_focus() -> bool {
    #[cfg(target_os = "windows")]
    {
        windows_impl::reassert()
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// Whether a held borrow went stale because the focus moved elsewhere.
pub fn search_focus_lost() -> bool {
    #[cfg(target_os = "windows")]
    {
        windows_impl::lost()
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ime_updates_are_only_reported_when_something_changed() {
        reset_ime_text();
        assert_eq!(take_ime_update(), None);

        {
            let mut text = ime_text().lock().expect("ime text");
            text.composition = "gong".to_string();
            text.dirty = true;
        }
        assert_eq!(
            take_ime_update(),
            Some(ImeUpdate {
                composition: "gong".to_string(),
                committed: String::new(),
            })
        );
        // Drained: the unchanged state is not reported twice.
        assert_eq!(take_ime_update(), None);
    }

    #[test]
    fn committed_text_is_drained_while_the_composition_stays() {
        reset_ime_text();
        {
            let mut text = ime_text().lock().expect("ime text");
            push_ime_unit(&mut text, '工' as u16);
            text.composition = "zuo".to_string();
            text.dirty = true;
        }
        assert_eq!(
            take_ime_update(),
            Some(ImeUpdate {
                composition: "zuo".to_string(),
                committed: "工".to_string(),
            })
        );
        assert_eq!(
            take_ime_update(),
            None,
            "an unchanged composition is not resubmitted"
        );

        reset_ime_text();
        assert_eq!(take_ime_update(), None);
    }

    #[test]
    fn surrogate_pairs_are_reassembled() {
        reset_ime_text();
        {
            let mut text = ime_text().lock().expect("ime text");
            push_ime_unit(&mut text, 0xD83D);
            // A lone high surrogate is not text yet.
            assert!(text.committed.is_empty());
            push_ime_unit(&mut text, 0xDE00);
        }
        assert_eq!(
            take_ime_update(),
            Some(ImeUpdate {
                composition: String::new(),
                committed: "😀".to_string(),
            })
        );
    }
}
