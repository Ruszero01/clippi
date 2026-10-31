//! One-shot focus snapshot for the activating main window. The quick window
//! never uses this path. UI Automation objects stay on their own COM thread.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, IsWindow,
};

use super::focus::{keyboard_focus_window, live_input_owner, root_window};

#[derive(Clone, Copy)]
pub(super) struct Snapshot {
    id: u64,
    root: usize,
    focus: usize,
    root_pid: u32,
    focus_pid: u32,
    automation_id: Option<u64>,
}

static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static ACTIVE_ID: AtomicU64 = AtomicU64::new(0);

enum Request {
    Capture {
        id: u64,
        root: usize,
        focus: usize,
        reply: mpsc::SyncSender<bool>,
    },
    Restore {
        id: u64,
        root: usize,
        root_pid: u32,
        deadline: Instant,
        reply: mpsc::SyncSender<bool>,
    },
}

fn automation_worker() -> &'static mpsc::Sender<Request> {
    static WORKER: OnceLock<mpsc::Sender<Request>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            use windows::Win32::System::Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
                COINIT_MULTITHREADED,
            };
            use windows::Win32::UI::Accessibility::{
                CUIAutomation, IUIAutomation, IUIAutomationElement,
            };
            // COM interfaces never leave this apartment.
            unsafe {
                if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
                    return;
                }
                let automation: Option<IUIAutomation> =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok();
                let mut saved: Option<(u64, IUIAutomationElement)> = None;
                while let Ok(request) = rx.recv() {
                    match request {
                        Request::Capture {
                            id,
                            root,
                            focus,
                            reply,
                        } => {
                            saved = None;
                            let element =
                                automation.as_ref().and_then(|a| a.GetFocusedElement().ok());
                            let valid = current_root() == root
                                && keyboard_focus_window().map(|h| h as usize) == Some(focus);
                            if valid {
                                if let Some(element) = element {
                                    saved = Some((id, element));
                                }
                            }
                            let _ = reply.send(saved.is_some());
                        }
                        Request::Restore {
                            id,
                            root,
                            root_pid,
                            deadline,
                            reply,
                        } => {
                            let element =
                                if saved.as_ref().is_some_and(|(saved_id, _)| *saved_id == id) {
                                    saved.take()
                                } else {
                                    None
                                };
                            let ok = if ACTIVE_ID.load(Ordering::SeqCst) == id
                                && Instant::now() < deadline
                                && current_root() == root
                                && window_pid(root as HWND) == root_pid
                            {
                                element.is_some_and(|(_, element)| {
                                    element.SetFocus().is_ok()
                                        && element
                                            .CurrentHasKeyboardFocus()
                                            .is_ok_and(|v| v.as_bool())
                                })
                            } else {
                                false
                            };
                            let _ = reply.send(ok);
                        }
                    }
                }
                drop(saved);
                drop(automation);
                CoUninitialize();
            }
        });
        tx
    })
}

fn current_root() -> usize {
    root_window(unsafe { GetForegroundWindow() }) as usize
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid
}

fn matches_snapshot(
    expected_root: usize,
    expected_pid: u32,
    target_root: usize,
    foreground_root: usize,
    actual_pid: u32,
) -> bool {
    expected_root != 0
        && expected_pid != 0
        && expected_root == target_root
        && expected_root == foreground_root
        && expected_pid == actual_pid
}

/// Capture before the main window is activated; discard a timed-out UIA query.
pub(crate) fn capture() {
    clear();
    ACTIVE_ID.store(0, Ordering::SeqCst);
    let Some(owner) = live_input_owner() else {
        return;
    };
    let Some(focus) = keyboard_focus_window() else {
        return;
    };
    let root = root_window(owner) as usize;
    let root_pid = window_pid(root as HWND);
    let focus_pid = window_pid(focus);
    if root == 0
        || root_pid == 0
        || focus_pid == 0
        || root_window(focus) as usize != root
        || current_root() != root
    {
        return;
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::sync_channel(1);
    let sent = automation_worker()
        .send(Request::Capture {
            id,
            root,
            focus: focus as usize,
            reply: tx,
        })
        .is_ok();
    let automation_id =
        (sent && rx.recv_timeout(Duration::from_millis(150)).unwrap_or(false)).then_some(id);
    if current_root() != root || keyboard_focus_window().map(|h| h as usize) != Some(focus as usize)
    {
        return;
    }
    ACTIVE_ID.store(id, Ordering::SeqCst);
    *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Snapshot {
        id,
        root,
        focus: focus as usize,
        root_pid,
        focus_pid,
        automation_id,
    });
}

pub(crate) fn clear() {
    *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Invalidate in-flight main-window pastes before opening the quick window.
pub(crate) fn cancel() {
    ACTIVE_ID.store(0, Ordering::SeqCst);
    clear();
}

pub(super) fn take() -> Option<Snapshot> {
    SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Called by the paste worker after the foreground has settled. Never move
/// focus back to an application the user has since switched away from.
pub(super) fn restore(snapshot: Snapshot, target: HWND) -> bool {
    if ACTIVE_ID.load(Ordering::SeqCst) != snapshot.id
        || !matches_snapshot(
            snapshot.root,
            snapshot.root_pid,
            root_window(target) as usize,
            current_root(),
            window_pid(snapshot.root as HWND),
        )
    {
        return false;
    }
    let child = snapshot.focus as HWND;
    unsafe {
        if IsWindow(child) != 0
            && root_window(child) as usize == snapshot.root
            && window_pid(child) == snapshot.focus_pid
        {
            let thread = GetWindowThreadProcessId(child, std::ptr::null_mut());
            let current = GetCurrentThreadId();
            let attached =
                thread != 0 && thread != current && AttachThreadInput(current, thread, 1) != 0;
            if thread == current || attached {
                SetFocus(child);
            }
            if attached {
                AttachThreadInput(current, thread, 0);
            }
        }
    }
    if let Some(id) = snapshot.automation_id {
        let (tx, rx) = mpsc::sync_channel(1);
        let deadline = Instant::now() + Duration::from_millis(300);
        if automation_worker()
            .send(Request::Restore {
                id,
                root: snapshot.root,
                root_pid: snapshot.root_pid,
                deadline,
                reply: tx,
            })
            .is_ok()
        {
            match rx.recv_timeout(Duration::from_millis(300)) {
                Ok(true) => log::debug!("Restored UI Automation focus"),
                Ok(false) => log::debug!("Could not verify UI Automation focus restoration"),
                Err(_) => {
                    log::warn!("UI Automation focus restoration timed out");
                    return false;
                }
            }
        }
    }
    ACTIVE_ID.load(Ordering::SeqCst) == snapshot.id
}

#[cfg(test)]
mod tests {
    use super::matches_snapshot;

    #[test]
    fn restore_requires_the_original_target_and_process() {
        assert!(matches_snapshot(1, 2, 1, 1, 2));
        assert!(!matches_snapshot(1, 2, 3, 1, 2));
        assert!(!matches_snapshot(1, 2, 1, 3, 2));
        assert!(!matches_snapshot(1, 2, 1, 1, 3));
        assert!(!matches_snapshot(0, 2, 0, 0, 2));
        assert!(!matches_snapshot(1, 0, 1, 1, 0));
    }
}
