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
//! The search state needs no keyboard focus. Keys are translated here and queued
//! for the popup, so opening it never moves the caret away from the application
//! the user is typing in, and a launcher palette never reads a search as "the
//! user clicked outside me" and dismisses itself.
//!
//! The input method is the one thing that cannot work without the focus, so it is
//! a separate, explicit action (`ui::quick_paste::take_input_method`, bound to a
//! double click on the search box) that borrows the focus for as long as the user
//! asked for it and no longer.
//!
//! Priority order, highest first:
//! 1. An input method that is composing keeps every key — pinyin typing and,
//!    above all, digit candidate selection. That is the composition of the
//!    application underneath; the popup's own composition, which needs the input
//!    method to have been taken over, is rule 2.
//! 2. A search box that has taken the input method over
//!    (`platform::quick_focus`) hands the text keys to it, so the composition —
//!    and with it the candidate list — is built inside the popup instead of
//!    being turned into latin letters here. The input method's own switch key is
//!    handed over as well, so a composition can be turned on and off again.
//! 3. The popup's own shortcuts: navigation, paste, close, slot digits, and the
//!    Tab that turns the search state on and off.
//! 4. A search box in its text-entry state (`SEARCH_ARMED`) takes every plain
//!    key: the slot digits step aside for search input, and nothing the user
//!    types reaches the application underneath while the search is under way.
//! 5. Everything else without Ctrl/Alt/Win becomes search text.
//!
//! Shortcuts with Ctrl, Alt or Win are never claimed, so the application
//! underneath still receives Ctrl+V and the like: entering the search state
//! takes the typing, not the system's shortcuts. Everything goes back to the
//! application the moment the search state ends.
//!
//! A takeover that cannot be proven — the popup does not really receive the keys
//! — costs the input method rather than the search: the keys are translated here
//! instead, and still nothing is handed to the application underneath.

use std::collections::VecDeque;
#[cfg(target_os = "windows")]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Turn the search box on, or off again: the state that takes the typing
    /// away from the application underneath and turns the slot digits into
    /// search text.
    SearchToggle,
    Char(char),
    Backspace,
}

/// Whether claimed keys are delivered to the quick popup (or dropped).
static ENABLED: AtomicBool = AtomicBool::new(false);
/// Mirrors "the quick search box has no text yet" so the hook can decide whether
/// a digit picks a slot or belongs to the query. Updated by the poll loop.
static QUERY_EMPTY: AtomicBool = AtomicBool::new(true);
/// Whether the search box is in its text-entry state: the state that takes the
/// typing away from the application underneath (click, Tab, or the
/// `auto_focus_search` setting). Held without the keyboard focus, because the
/// keys the search box needs are translated here.
static SEARCH_ARMED: AtomicBool = AtomicBool::new(false);
/// Whether the popup has borrowed the keyboard focus for its input method
/// (`platform::quick_focus`). Held only while the popup can really receive keys,
/// which is the condition for handing text keys over to it.
static INPUT_METHOD_TAKEN: AtomicBool = AtomicBool::new(false);
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

/// Record whether the search box is in its text-entry state.
///
/// While it is, every plain key belongs to the popup and none of them reaches
/// the application underneath. The state is held without the keyboard focus:
/// the keys are translated by the hook itself, so nothing has to be taken away
/// from the application the user is typing in.
pub fn set_search_armed(armed: bool) {
    SEARCH_ARMED.store(armed, Ordering::SeqCst);
}

/// Whether the search box is in its text-entry state.
pub fn is_search_armed() -> bool {
    SEARCH_ARMED.load(Ordering::SeqCst)
}

/// Record whether the popup has borrowed the keyboard focus for its input
/// method.
///
/// While it has, text keys belong to the focused popup: the input method has to
/// see them to build a composition, so the hook hands them over instead of
/// translating them itself.
pub fn set_input_method_taken(taken: bool) {
    INPUT_METHOD_TAKEN.store(taken, Ordering::SeqCst);
}

/// Whether the popup has borrowed the keyboard focus for its input method.
pub fn is_input_method_taken() -> bool {
    INPUT_METHOD_TAKEN.load(Ordering::SeqCst)
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
    /// While the popup holds the keyboard focus on loan, only its own input
    /// method context counts. What the application underneath is composing is
    /// irrelevant — the keys belong to the search box either way — and reading it
    /// there would hand the keys to that application the moment it starts a
    /// composition of its own, which is exactly what leaves the search box unable
    /// to be typed into.
    fn input_method_state(input_method_taken: bool) -> (bool, bool) {
        if input_method_taken {
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
    /// switch. The search box claims every key that turns into no character, and
    /// claiming this one would make entering a composition a one-way trip: the
    /// Shift that switches the input method out would also be the Shift that can
    /// never switch it back in, leaving the search box stuck in latin with no way
    /// back to Chinese.
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
        // Whether the popup is taking the typing — either its search box is in
        // its text-entry state, or it has taken the input method over — and what
        // its input method is doing. The input method is read from the popup
        // while it holds the borrowed focus, so the application underneath cannot
        // take the keys back by starting a composition of its own.
        let search_editing = is_search_armed() || is_input_method_taken();
        let (ime_open, ime_composing) = input_method_state(is_input_method_taken());
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

        // A search box in its text-entry state owns every typed key: the
        // application underneath must not see a single one of them while the
        // user is searching.
        //
        // The keys only change hands once the popup has taken the input method
        // over — then the input method has to see them to build a composition in
        // the search box (translating pinyin here would turn it into latin
        // letters). In every other case they are translated here and land in the
        // search box, so neither an application that takes its focus back nor an
        // input method that never started can swallow the search. Composing
        // already returned above, so this covers the keys that start one.
        let popup_holds_keys =
            is_input_method_taken() && crate::platform::quick_focus::popup_has_keyboard_focus();
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
            // Tab turns the search box on, or off again. It has to be reachable
            // without a mouse: the launcher palettes this popup pastes into
            // (Listary, Quicker) dismiss themselves when the user clicks outside
            // them, so a click on the search box costs the user the very target
            // the search was for. A one-line search box has no other use for Tab.
            VK_TAB if !ctrl && !alt && !win => Some(QuickKey::SearchToggle),
            // Digit keys pick a slot only while the popup is being used without
            // typing into the search box; in a search box that is taking the
            // typing they are search input like any other character (handled by
            // the character arm below), so a digit can never dismiss a search
            // under way. The numpad behaves like the main row so "1 presses the
            // first entry" holds for both digit clusters.
            0x31..=0x39 | 0x61..=0x69
                if !ctrl
                    && !alt
                    && !win
                    && !search_editing
                    && QUERY_EMPTY.load(Ordering::SeqCst) =>
            {
                let first_digit = if vk >= 0x61 { 0x61 } else { 0x31 };
                Some(QuickKey::Pick((vk - first_digit) as usize))
            }
            // A search box that is taking the typing owns every plain key:
            // whatever a key translates to is search text, and a key that
            // translates to nothing (Home, Delete, …) has no meaning in a
            // one-line search box either. Neither may reach the application
            // underneath — that leak is what this state exists to close. While
            // the input method has been taken over, the keys are handed to it
            // instead, so the composition happens in the search box, and so is
            // the input method's own switch so the user can turn the composition
            // on and off again at will.
            _ if search_editing && !ctrl && !alt && !win => {
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

#[cfg(target_os = "macos")]
mod macos_impl {
    use super::*;
    use core_foundation::runloop::{kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFRunLoop};
    use core_graphics::event::{
        CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions,
        CGEventTapPlacement, CGEventType, CallbackResult, EventField, KeyCode,
    };
    use foreign_types::ForeignType;
    use std::collections::HashSet;
    use std::os::raw::c_ulong;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::thread::JoinHandle;
    use std::time::Duration;

    static INSTALLED: AtomicBool = AtomicBool::new(false);
    static RUNNING: AtomicBool = AtomicBool::new(false);
    static REENABLE: AtomicBool = AtomicBool::new(false);
    static THREAD: OnceLock<Mutex<Option<JoinHandle<()>>>> = OnceLock::new();
    static CLAIMED: OnceLock<Mutex<HashSet<u16>>> = OnceLock::new();

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventKeyboardGetUnicodeString(
            event: core_graphics::sys::CGEventRef,
            max_length: c_ulong,
            actual_length: *mut c_ulong,
            buffer: *mut u16,
        );
    }

    fn unicode_text(event: &CGEvent) -> String {
        let mut buffer = [0u16; 8];
        let mut length = 0;
        // SAFETY: CoreGraphics writes at most `buffer.len()` UTF-16 units into
        // the stack buffer. `event` remains valid for the callback's duration.
        unsafe {
            CGEventKeyboardGetUnicodeString(
                event.as_ptr(),
                buffer.len() as c_ulong,
                &mut length,
                buffer.as_mut_ptr(),
            )
        };
        String::from_utf16_lossy(&buffer[..(length as usize).min(buffer.len())])
    }

    fn claimed_keys() -> &'static Mutex<HashSet<u16>> {
        CLAIMED.get_or_init(|| Mutex::new(HashSet::new()))
    }

    fn process(event_type: CGEventType, event: &CGEvent) -> CallbackResult {
        if matches!(
            event_type,
            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
        ) {
            REENABLE.store(true, Ordering::SeqCst);
            return CallbackResult::Keep;
        }
        let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
        if matches!(event_type, CGEventType::KeyUp) {
            return if claimed_keys()
                .lock()
                .is_ok_and(|mut keys| keys.remove(&keycode))
            {
                CallbackResult::Drop
            } else {
                CallbackResult::Keep
            };
        }
        if !matches!(event_type, CGEventType::KeyDown) || !is_enabled() {
            return CallbackResult::Keep;
        }
        if event.get_integer_value_field(EventField::EVENT_SOURCE_UNIX_PROCESS_ID)
            == std::process::id() as i64
        {
            // Paste injection originates in Clippi and must reach its target.
            return CallbackResult::Keep;
        }

        let flags = event.get_flags();
        if flags.intersects(
            CGEventFlags::CGEventFlagControl
                | CGEventFlags::CGEventFlagAlternate
                | CGEventFlags::CGEventFlagCommand,
        ) {
            return CallbackResult::Keep;
        }

        let key = match keycode {
            KeyCode::UP_ARROW => Some(QuickKey::Previous),
            KeyCode::DOWN_ARROW => Some(QuickKey::Next),
            KeyCode::LEFT_ARROW => Some(QuickKey::PreviousPage),
            KeyCode::RIGHT_ARROW => Some(QuickKey::NextPage),
            KeyCode::RETURN | KeyCode::ANSI_KEYPAD_ENTER => Some(QuickKey::Paste),
            KeyCode::ESCAPE => Some(QuickKey::Close),
            KeyCode::TAB => Some(QuickKey::SearchToggle),
            KeyCode::DELETE => Some(QuickKey::Backspace),
            _ => None,
        };
        let keys = if let Some(key) = key {
            vec![key]
        } else {
            let value = unicode_text(event);
            if value.is_empty() || value.chars().any(char::is_control) {
                if is_search_armed() {
                    Vec::new()
                } else {
                    return CallbackResult::Keep;
                }
            } else if !is_search_armed()
                && QUERY_EMPTY.load(Ordering::SeqCst)
                && value.len() == 1
                && value.as_bytes()[0].is_ascii_digit()
                && value != "0"
            {
                vec![QuickKey::Pick((value.as_bytes()[0] - b'1') as usize)]
            } else {
                value.chars().map(QuickKey::Char).collect()
            }
        };

        // A poisoned queue must not eat a user's keystroke. Once queued, keep
        // its key-up away from the foreground app as well.
        let Ok(mut queue) = events().lock() else {
            return CallbackResult::Keep;
        };
        queue.extend(keys);
        if let Ok(mut claimed) = claimed_keys().lock() {
            claimed.insert(keycode);
        }
        CallbackResult::Drop
    }

    pub fn install() -> Result<(), String> {
        let slot = THREAD.get_or_init(|| Mutex::new(None));
        let mut thread = slot.lock().map_err(|_| "keyboard hook lock poisoned")?;
        if INSTALLED.load(Ordering::SeqCst) {
            return Ok(());
        }
        RUNNING.store(true, Ordering::SeqCst);
        let (sender, receiver) = mpsc::sync_channel(1);
        let handle = std::thread::spawn(move || {
            let tap = CGEventTap::new(
                CGEventTapLocation::Session,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                vec![CGEventType::KeyDown, CGEventType::KeyUp],
                |_proxy, event_type, event| process(event_type, event),
            );
            let Ok(tap) = tap else {
                let _ = sender.send(false);
                return;
            };
            let Ok(source) = tap.mach_port().create_runloop_source(0) else {
                let _ = sender.send(false);
                return;
            };
            CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
            tap.enable();
            INSTALLED.store(true, Ordering::SeqCst);
            let _ = sender.send(true);
            while RUNNING.load(Ordering::SeqCst) {
                CFRunLoop::run_in_mode(
                    unsafe { kCFRunLoopDefaultMode },
                    Duration::from_millis(100),
                    false,
                );
                if REENABLE.swap(false, Ordering::SeqCst) {
                    tap.enable();
                }
            }
            INSTALLED.store(false, Ordering::SeqCst);
        });
        let installed = receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or(false);
        *thread = Some(handle);
        if installed {
            Ok(())
        } else {
            RUNNING.store(false, Ordering::SeqCst);
            if let Some(handle) = thread.take() {
                let _ = handle.join();
            }
            Err("macOS keyboard event tap unavailable (check Accessibility permission)".into())
        }
    }

    pub fn uninstall() {
        RUNNING.store(false, Ordering::SeqCst);
        if let Some(slot) = THREAD.get() {
            if let Ok(mut thread) = slot.lock() {
                if let Some(handle) = thread.take() {
                    let _ = handle.join();
                }
            }
        }
    }

    pub fn is_installed() -> bool {
        INSTALLED.load(Ordering::SeqCst)
    }
}

/// Install the desktop-wide keyboard hook used by the quick popup.
#[cfg(target_os = "windows")]
pub fn install() -> Result<(), String> {
    windows_impl::install()
}

#[cfg(target_os = "macos")]
pub fn install() -> Result<(), String> {
    macos_impl::install()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn install() -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn uninstall() {
    windows_impl::uninstall()
}

#[cfg(target_os = "macos")]
pub fn uninstall() {
    macos_impl::uninstall()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn uninstall() {}

/// Whether the hook is installed and can drive the quick popup.
#[cfg(target_os = "windows")]
pub fn is_available() -> bool {
    windows_impl::is_installed()
}

#[cfg(target_os = "macos")]
pub fn is_available() -> bool {
    macos_impl::is_installed()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
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

    /// The search state and the input-method takeover are independent: entering
    /// the search box must not be the same thing as taking the keyboard focus
    /// away from the application underneath.
    #[test]
    fn search_state_and_input_method_takeover_are_tracked_separately() {
        set_search_armed(true);
        assert!(is_search_armed());
        assert!(!is_input_method_taken());

        set_input_method_taken(true);
        set_search_armed(false);
        assert!(!is_search_armed());
        assert!(is_input_method_taken());

        set_input_method_taken(false);
        assert!(!is_input_method_taken());
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
