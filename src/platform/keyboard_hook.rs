//! Low-level keyboard hook for the quick paste popup.
//!
//! The quick popup is a `WS_EX_NOACTIVATE` window: it must never take focus away
//! from the application the user is typing in, which also means it cannot
//! receive keyboard input the normal way. While the popup is visible a
//! `WH_KEYBOARD_LL` hook observes keystrokes before any window sees them, claims
//! the ones that belong to the popup (navigation, paste, digits, plain text,
//! backspace) and lets everything else through — modified shortcuts, and above
//! all anything an input method is composing.
//!
//! Claimed keys are queued here and drained by the window manager's fast poll on
//! the GPUI thread; this module never touches GPUI state.
//!
//! Priority order, highest first:
//! 1. An input method that is composing keeps every key — pinyin typing and,
//!    above all, digit candidate selection.
//! 2. An armed search box (the user clicked it, pressed Tab, or the
//!    `auto_focus_search` setting armed it on show; `platform::quick_focus`
//!    holds the keys) takes every plain key: its own shortcuts and all search
//!    text,
//!    and nothing the user types reaches the application underneath while it is
//!    armed. The slot digits step aside for search input, and the input
//!    method's own switch key is handed over so a composition can be turned on
//!    and off again inside the search box.
//! 3. The popup's own shortcuts: navigation, paste, close, slot digits, and the
//!    Tab that hands the keyboard focus to the search box and back again.
//! 4. Everything else without Ctrl/Alt/Win becomes search text.
//!
//! Shortcuts with Ctrl, Alt or Win are never claimed, so the application
//! underneath still receives Ctrl+V and the like: arming the search box takes
//! the typing, not the system's shortcuts. Everything goes back to the
//! application the moment the search state ends.
//!
//! Once the search box is given the keyboard — a click, the Tab that exists
//! because a click outside dismisses the launcher palettes this popup pastes
//! into, or the automatic focus on show — the popup borrows the keyboard focus
//! (`platform::quick_focus`) and the text keys change hands again: while the
//! input method is open *and* the popup really receives the keys, they are
//! passed through untouched, so the composition — and with it the candidate
//! list — happens in the popup instead of being turned into latin text here.
//! Whenever that cannot be proven, they are translated here instead, so a
//! takeover that did not take effect costs the input method rather than the
//! search, and still hands nothing to the application underneath.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// A key that the quick popup takes ownership of.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickKey {
    Previous,
    Next,
    PreviousPage,
    NextPage,
    /// Shift pastes as plain text, Ctrl opens the advanced paste modes.
    Paste,
    Close,
    /// 0-based slot for the digit keys `1`–`9`.
    Pick(usize),
    /// Hand the keyboard focus to the search box, or take it back: the arm the
    /// input method needs, reachable without a mouse click.
    SearchFocus,
    Char(char),
    Backspace,
}

/// Whether claimed keys are delivered to the quick popup (or dropped).
static ENABLED: AtomicBool = AtomicBool::new(false);
/// Mirrors "the quick search box has no text yet" so the hook can decide whether
/// a digit picks a slot or belongs to the query. Updated by the poll loop.
static QUERY_EMPTY: AtomicBool = AtomicBool::new(true);
/// Whether the popup currently owns the keyboard focus because the user clicked
/// the search box (`platform::quick_focus`). Held only while the popup can
/// really receive keys, so text keys may be handed over to it.
static SEARCH_FOCUSED: AtomicBool = AtomicBool::new(false);
static EVENTS: OnceLock<Mutex<VecDeque<QuickKey>>> = OnceLock::new();

fn events() -> &'static Mutex<VecDeque<QuickKey>> {
    EVENTS.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Enable or disable key claiming.
///
/// Disabled, every key reaches the window it was meant for.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::SeqCst);
    if !enabled {
        // Drop queued keys so the next session cannot replay them. Claimed keys
        // are kept on purpose: the popup is normally dismissed by the key that
        // is still held down (Enter), and that press's release has to stay
        // swallowed after the popup is gone. See `keyboard_proc`.
        if let Ok(mut queue) = events().lock() {
            queue.clear();
        }
    }
}

/// Whether the hook is currently claiming keys.
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// Record whether the quick search box currently holds no text.
pub fn set_query_empty(empty: bool) {
    QUERY_EMPTY.store(empty, Ordering::SeqCst);
}

/// Record whether the popup owns the keyboard focus for its search box.
///
/// While it does, text keys belong to the focused popup: the input method has
/// to see them to build a composition, so the hook hands them over instead of
/// translating them itself.
pub fn set_search_focused(focused: bool) {
    SEARCH_FOCUSED.store(focused, Ordering::SeqCst);
}

/// Whether the popup owns the keyboard focus for its search box.
pub fn is_search_focused() -> bool {
    SEARCH_FOCUSED.load(Ordering::SeqCst)
}

/// Drain the keys the hook claimed since the last call.
pub fn take_events() -> Vec<QuickKey> {
    events()
        .lock()
        .map(|mut queue| queue.drain(..).collect())
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::*;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::Ime::{
        ImmGetCompositionStringW, ImmGetContext, ImmGetOpenStatus, ImmReleaseContext, GCS_COMPSTR,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, GetKeyboardLayout, ToUnicodeEx, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DOWN,
        VK_ESCAPE, VK_LEFT, VK_LSHIFT, VK_LWIN, VK_MENU, VK_NEXT, VK_PRIOR, VK_PROCESSKEY,
        VK_RETURN, VK_RIGHT, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_TAB, VK_UP,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetForegroundWindow, GetGUIThreadInfo, GetMessageW,
        GetWindowThreadProcessId, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
        GUITHREADINFO, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, LLKHF_UP, MSG,
        WH_KEYBOARD_LL, WM_QUIT,
    };

    static HOOK: AtomicUsize = AtomicUsize::new(0);
    static HOOK_THREAD: AtomicUsize = AtomicUsize::new(0);
    /// Virtual keys whose key-down was claimed, so their key-up is swallowed
    /// too instead of reaching the target window as a stray release.
    static CLAIMED: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();

    fn claimed() -> &'static Mutex<Vec<u32>> {
        CLAIMED.get_or_init(|| Mutex::new(Vec::new()))
    }

    pub fn install() -> Result<(), String> {
        if HOOK.load(Ordering::SeqCst) != 0 {
            return Ok(());
        }
        let (tx, rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(0);

        std::thread::spawn(move || {
            // SAFETY: `GetCurrentThreadId` returns a constant thread id, and
            // `SetWindowsHookExW` with `WH_KEYBOARD_LL` and a null module handle
            // installs a desktop-wide hook whose callback lives in this binary.
            let thread_id = unsafe { GetCurrentThreadId() };
            let hook = unsafe {
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), std::ptr::null_mut(), 0)
            };
            if hook.is_null() {
                let _ = tx.send(Err(
                    "SetWindowsHookExW(WH_KEYBOARD_LL) failed; falling back to hotkeys".to_string(),
                ));
                return;
            }
            HOOK.store(hook as usize, Ordering::SeqCst);
            HOOK_THREAD.store(thread_id as usize, Ordering::SeqCst);
            let _ = tx.send(Ok(()));

            // Low-level hooks are dispatched through the installing thread's
            // message queue, so this thread has to keep pumping. `WM_QUIT`
            // (posted by `uninstall`) ends the loop.
            let mut message: MSG = unsafe { std::mem::zeroed() };
            // SAFETY: standard blocking message pump; the hook callbacks are
            // delivered while `GetMessageW` waits.
            while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {}

            // SAFETY: `hook` came from `SetWindowsHookExW` above and is removed
            // exactly once.
            unsafe { UnhookWindowsHookEx(hook) };
            HOOK.store(0, Ordering::SeqCst);
            HOOK_THREAD.store(0, Ordering::SeqCst);
        });

        match rx.recv() {
            Ok(result) => result,
            Err(_) => Err("keyboard hook thread exited before installing".to_string()),
        }
    }

    pub fn uninstall() {
        let thread_id = HOOK_THREAD.load(Ordering::SeqCst) as u32;
        if thread_id != 0 {
            // SAFETY: `PostThreadMessageW` only posts `WM_QUIT` to the hook
            // thread's queue; the thread stays alive until it processes it.
            unsafe { PostThreadMessageW(thread_id, WM_QUIT, 0, 0) };
        }
    }

    pub fn is_installed() -> bool {
        HOOK.load(Ordering::SeqCst) != 0
    }

    fn push(key: QuickKey) {
        if let Ok(mut queue) = events().lock() {
            queue.push_back(key);
        }
    }

    fn is_down(vk: u16) -> bool {
        // SAFETY: `GetAsyncKeyState` only reads the state of a known key code.
        unsafe { GetAsyncKeyState(vk as i32) < 0 }
    }

    /// Window that currently owns the keyboard focus.
    ///
    /// Launcher palettes and other non-activating popups take keyboard focus
    /// without ever becoming the foreground window, so the foreground window's
    /// thread focus — not the foreground window itself — is the authoritative
    /// source for both the input method state and the keyboard layout.
    fn focus_window() -> Option<HWND> {
        // SAFETY: read-only queries; `GUITHREADINFO` is zero-initialised with
        // `cbSize` set as the API requires.
        unsafe {
            let foreground = GetForegroundWindow();
            if foreground.is_null() {
                return None;
            }
            let thread = GetWindowThreadProcessId(foreground, std::ptr::null_mut());
            if thread != 0 {
                let mut info: GUITHREADINFO = std::mem::zeroed();
                info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
                if GetGUIThreadInfo(thread, &mut info) != 0 && !info.hwndFocus.is_null() {
                    return Some(info.hwndFocus);
                }
            }
            Some(foreground)
        }
    }

    /// Whether an input method is composing for the focused window.
    ///
    /// While a composition is open every key belongs to the IME: pinyin typing
    /// and — most importantly — digit candidate selection must not be
    /// intercepted by the popup. Losing that keystroke would silently break the
    /// user's candidate choice, so the input method always wins over the
    /// popup's digit shortcuts.
    fn ime_is_composing() -> bool {
        let Some(target) = focus_window() else {
            return false;
        };
        // SAFETY: read-only IMM queries on a window handle; the context handed
        // out by `ImmGetContext` is always released again.
        unsafe {
            let context = ImmGetContext(target);
            if context.is_null() {
                return false;
            }
            let composing =
                ImmGetCompositionStringW(context, GCS_COMPSTR, std::ptr::null_mut(), 0) > 0;
            ImmReleaseContext(target, context);
            composing
        }
    }

    /// Whether the input method is switched into its composing (Chinese,
    /// Japanese, …) mode for the focused window.
    ///
    /// A hook can translate one key at a time; a composition has to happen in
    /// the window that owns the focus. While the input method is open every text
    /// key has to reach that window — whether or not a composition is already on
    /// screen — so this is what tells the hook to keep its hands off text keys
    /// while the search box is focused.
    fn ime_is_open() -> bool {
        let Some(target) = focus_window() else {
            return false;
        };
        // SAFETY: read-only IMM query on a window handle; the context handed
        // out by `ImmGetContext` is always released again.
        unsafe {
            let context = ImmGetContext(target);
            if context.is_null() {
                return false;
            }
            let open = ImmGetOpenStatus(context) != 0;
            ImmReleaseContext(target, context);
            open
        }
    }

    /// Input method state that decides who owns the text keys: `(open,
    /// composing)`.
    ///
    /// While the search box holds the keyboard focus on loan, only the popup's
    /// own input method context counts. What the application underneath is
    /// composing is irrelevant — the keys belong to the search box either way —
    /// and reading it there would hand the keys to that application the moment
    /// it starts a composition of its own, which is exactly what leaves the
    /// search box unable to be typed into.
    fn input_method_state(search_box_holds_focus: bool) -> (bool, bool) {
        if search_box_holds_focus {
            if let Some(state) = crate::platform::quick_focus::search_ime_state() {
                return state;
            }
        }
        (ime_is_open(), ime_is_composing())
    }

    /// Whether the key exists only to switch the input method between its
    /// Chinese and latin mode, and produces no text of its own.
    ///
    /// That is the bare Shift, which every Chinese input method uses as its
    /// switch. An armed search box claims every key that turns into no
    /// character, and claiming this one would make entering a composition a
    /// one-way trip: the Shift that switches the input method out would also be
    /// the Shift that can never switch it back in, leaving the search box stuck
    /// in latin with no way back to Chinese.
    fn switches_input_method(vk: u16) -> bool {
        matches!(vk, VK_SHIFT | VK_LSHIFT | VK_RSHIFT)
    }

    /// Translate a key press into the character it would produce under the
    /// layout of the thread that owns the focused window.
    ///
    /// `ToUnicodeEx` is called with the "do not change keyboard state" flag so
    /// dead keys keep working in the application that owns them.
    fn char_for_key(vk: u16, scan_code: u32, extended: bool) -> Option<char> {
        // Extended keys carry the `0xE0` prefix in their scan code; the hook
        // reports the bare code, so put the prefix back before translating.
        let scan = if extended {
            scan_code | 0xE000
        } else {
            scan_code
        };
        let mut state = [0u8; 256];
        if is_down(VK_SHIFT) {
            state[VK_SHIFT as usize] = 0x80;
        }
        // SAFETY: the low bit of `GetAsyncKeyState` is the toggle state, which
        // is how Caps Lock has to be reported to `ToUnicodeEx`.
        if unsafe { GetAsyncKeyState(VK_CAPITAL as i32) } & 1 != 0 {
            state[VK_CAPITAL as usize] = 0x01;
        }

        let mut buffer = [0u16; 8];
        // SAFETY: `buffer` is a stack array of exactly the length passed along.
        // The layout belongs to the thread owning the focused window, so the
        // character matches what the application the user is typing in would
        // have received (and dead keys stay on that thread's layout).
        let written = unsafe {
            let thread = focus_window()
                .map(|target| GetWindowThreadProcessId(target, std::ptr::null_mut()))
                .unwrap_or(0);
            let layout = GetKeyboardLayout(thread);
            ToUnicodeEx(
                vk as u32,
                scan,
                state.as_ptr(),
                buffer.as_mut_ptr(),
                buffer.len() as i32,
                1,
                layout,
            )
        };
        if written != 1 {
            return None;
        }
        char::from_u32(buffer[0] as u32).filter(|character| !character.is_control())
    }

    /// Release a claimed key and report whether it was claimed.
    ///
    /// The press of a claimed key never reached the application, so its release
    /// must not reach it either — including a release that arrives after the
    /// popup was dismissed by that very key.
    fn unclaim(vk: u32) -> bool {
        claimed()
            .lock()
            .map(|mut keys| match keys.iter().position(|&key| key == vk) {
                Some(position) => {
                    keys.swap_remove(position);
                    true
                }
                None => false,
            })
            .unwrap_or(false)
    }

    /// Swallow a claimed key that means nothing to the popup.
    ///
    /// The press is dropped and its release is remembered, so the application
    /// underneath receives neither half of a key that was typed into the search
    /// box.
    fn claim_ignored(vk: u32) -> LRESULT {
        if let Ok(mut keys) = claimed().lock() {
            if !keys.contains(&vk) {
                keys.push(vk);
            }
        }
        1
    }

    /// Claim a key press: remember it so its release is swallowed as well.
    fn claim(vk: u32, key: QuickKey) -> LRESULT {
        if let Ok(mut keys) = claimed().lock() {
            if !keys.contains(&vk) {
                keys.push(vk);
            }
        }
        push(key);
        1
    }

    /// SAFETY: Win32 calls this with a valid `KBDLLHOOKSTRUCT` whenever
    /// `code >= 0`; the pointer is only read, never stored.
    unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let pass_through = || CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
        if code < 0 {
            return pass_through();
        }
        // SAFETY: for `code >= 0` the system passes a `KBDLLHOOKSTRUCT`.
        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        // Never touch our own injected input (the paste keystroke).
        if info.flags & LLKHF_INJECTED != 0 {
            return pass_through();
        }
        let key_up = info.flags & LLKHF_UP != 0;

        if !is_enabled() {
            // A disabled hook still swallows the release of a key it claimed:
            // the popup is normally dismissed by the key that is still held
            // down (Enter), so the application underneath must not receive that
            // press's release as a stray keystroke either.
            return if key_up && unclaim(info.vkCode) {
                1
            } else {
                pass_through()
            };
        }

        let vk = info.vkCode as u16;
        // Whether the popup owns the keys itself, and what its input method is
        // doing. Both are read from the popup while it holds the borrowed focus,
        // so the application underneath cannot take the keys back by starting a
        // composition of its own.
        let search_box_holds_focus = is_search_focused();
        let (ime_open, ime_composing) = input_method_state(search_box_holds_focus);
        // `VK_PROCESSKEY` means the IME is already handling this key.
        if vk == VK_PROCESSKEY || ime_composing {
            return pass_through();
        }

        if key_up {
            return if unclaim(info.vkCode) {
                1
            } else {
                pass_through()
            };
        }

        let ctrl = is_down(VK_CONTROL);
        let alt = is_down(VK_MENU);
        let win = is_down(VK_LWIN) || is_down(VK_RWIN);

        // An armed search box owns every typed key: the application underneath
        // must not see a single one of them while the user is searching.
        //
        // The keys only change hands when the popup really holds the keyboard
        // focus and its input method is open — then the input method has to see
        // them to build a composition in the search box (translating pinyin here
        // would turn it into latin letters). In every other case they are
        // translated here and land in the search box, so an application that
        // takes its focus back cannot swallow the search either way. Composing
        // already returned above, so this covers the keys that start one.
        let popup_holds_keys =
            search_box_holds_focus && crate::platform::quick_focus::popup_has_keyboard_focus();
        let ime_owns_text = popup_holds_keys && ime_open;

        let action = match vk {
            VK_UP => Some(QuickKey::Previous),
            VK_DOWN => Some(QuickKey::Next),
            VK_PRIOR => Some(QuickKey::PreviousPage),
            VK_NEXT => Some(QuickKey::NextPage),
            VK_LEFT if !alt && !win => Some(QuickKey::PreviousPage),
            VK_RIGHT if !alt && !win => Some(QuickKey::NextPage),
            VK_RETURN if !win => Some(QuickKey::Paste),
            VK_ESCAPE if !ctrl && !alt && !win => Some(QuickKey::Close),
            VK_BACK if !ctrl && !alt && !win => Some(QuickKey::Backspace),
            // Tab hands the search box the keyboard, or takes it back. Arming
            // has to be reachable without a mouse: the launcher palettes this
            // popup pastes into (Listary, Quicker) dismiss themselves when the
            // user clicks outside them, so a click on the search box costs the
            // user the very target the search was for. A one-line search box has
            // no other use for Tab, and a composition — the only thing that
            // might — already returned above.
            VK_TAB if !ctrl && !alt && !win => Some(QuickKey::SearchFocus),
            // Digit keys pick a slot only while the popup is being used without
            // typing into the search box; inside a focused search box they are
            // search input like any other character (handled by the character
            // arm below), so a digit can never dismiss a search under way. The
            // numpad behaves like the main row so "1 presses the first entry"
            // holds for both digit clusters.
            0x31..=0x39 | 0x61..=0x69
                if !ctrl
                    && !alt
                    && !win
                    && !search_box_holds_focus
                    && QUERY_EMPTY.load(Ordering::SeqCst) =>
            {
                let first_digit = if vk >= 0x61 { 0x61 } else { 0x31 };
                Some(QuickKey::Pick((vk - first_digit) as usize))
            }
            // The search box owns every plain key while it is armed: whatever a
            // key translates to is search text, and a key that translates to
            // nothing (Tab, Home, Delete, …) has no meaning in a one-line search
            // box either. Neither may reach the application underneath — that
            // leak is what this mode exists to close. While the input method is
            // open and the popup really holds the keys, they are handed over
            // instead, so the composition happens in the search box, and so is
            // the input method's own switch so the user can turn the composition
            // on and off again at will.
            _ if search_box_holds_focus && !ctrl && !alt && !win => {
                if ime_owns_text || (popup_holds_keys && switches_input_method(vk)) {
                    None
                } else if let Some(character) =
                    char_for_key(vk, info.scanCode, info.flags & LLKHF_EXTENDED != 0)
                {
                    Some(QuickKey::Char(character))
                } else {
                    return claim_ignored(info.vkCode);
                }
            }
            _ if !ctrl && !alt && !win => {
                char_for_key(vk, info.scanCode, info.flags & LLKHF_EXTENDED != 0)
                    .map(QuickKey::Char)
            }
            _ => None,
        };

        match action {
            Some(key) => claim(info.vkCode, key),
            None => pass_through(),
        }
    }
}

/// Install the desktop-wide keyboard hook (Windows only; elsewhere this is a
/// no-op and `is_available` reports `false`).
#[cfg(target_os = "windows")]
pub fn install() -> Result<(), String> {
    windows_impl::install()
}

#[cfg(not(target_os = "windows"))]
pub fn install() -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn uninstall() {
    windows_impl::uninstall()
}

#[cfg(not(target_os = "windows"))]
pub fn uninstall() {}

/// Whether the hook is installed and can drive the quick popup.
#[cfg(target_os = "windows")]
pub fn is_available() -> bool {
    windows_impl::is_installed()
}

#[cfg(not(target_os = "windows"))]
pub fn is_available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_is_empty_until_a_key_is_claimed() {
        assert!(take_events().is_empty());
        set_enabled(true);
        set_enabled(false);
        assert!(take_events().is_empty());
    }

    #[test]
    fn disabling_clears_pending_keys() {
        set_query_empty(true);
        assert!(QUERY_EMPTY.load(Ordering::SeqCst));
        set_query_empty(false);
        assert!(!QUERY_EMPTY.load(Ordering::SeqCst));
        set_enabled(false);
        assert!(!is_enabled());
    }
}
