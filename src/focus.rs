//! Focus 事件监听模块
//! 使用 Win32 SetWinEventHook 实现事件驱动的焦点监听
//! 跨平台支持：非 Windows 平台提供空实现

use std::sync::mpsc;

#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::{CloseHandle, HWND, TRUE};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK, WINEVENTPROC};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, PeekMessageW, PostThreadMessageW, TranslateMessage,
    DispatchMessageW, EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
    MSG, PM_REMOVE, WM_QUIT,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

/// 前台窗口变化事件
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusEvent {
    /// 前台窗口变化，参数为新前台进程的进程名（小写，None 表示无法获取）
    ForegroundChanged(Option<String>),
}

/// FocusWatcher 句柄
pub struct FocusWatcher {
    #[cfg(target_os = "windows")]
    hook: HWINEVENTHOOK,
    #[cfg(target_os = "windows")]
    thread: Option<std::thread::JoinHandle<()>>,
    #[cfg(target_os = "windows")]
    thread_id: u32,
}

impl FocusWatcher {
    /// 停止焦点监听
    #[cfg(target_os = "windows")]
    pub fn stop(&mut self) {
        unsafe { UnhookWinEvent(self.hook) };
        // 发送 WM_QUIT 到消息循环线程，唤醒 PeekMessageW 并使其退出
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub fn stop(&mut self) {}
}

/// 启动焦点监听，返回 FocusWatcher 和事件接收器
pub fn start_focus_watcher() -> Result<(FocusWatcher, mpsc::Receiver<FocusEvent>), String> {
    #[cfg(target_os = "windows")]
    {
        use std::sync::Arc;
        use std::sync::atomic::AtomicU32;
        use std::sync::atomic::Ordering;

        let (tx, rx) = mpsc::channel();
        unsafe { FOCUS_EVENT_TX = Some(tx) };

        // 用于在线程内部获取 thread_id
        let thread_id = Arc::new(AtomicU32::new(0));

        let hook = unsafe {
            let proc: WINEVENTPROC = Some(std::mem::transmute(win_event_proc as *const ()));
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                0 as *mut std::ffi::c_void,
                proc,
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };

        if hook.is_null() {
            return Err("SetWinEventHook failed".to_string());
        }

        let tid_clone = thread_id.clone();
        let thread = std::thread::spawn(move || {
            let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
            tid_clone.store(tid, Ordering::SeqCst);
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            loop {
                let ret = unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) };
                if ret == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                if msg.message == WM_QUIT {
                    break;
                }
                unsafe { TranslateMessage(&msg) };
                unsafe { DispatchMessageW(&msg) };
            }
        });

        // 等待线程启动并设置 thread_id
        let thread_id_val = loop {
            let tid = thread_id.load(Ordering::SeqCst);
            if tid != 0 {
                break tid;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        };

        Ok((FocusWatcher { hook, thread: Some(thread), thread_id: thread_id_val }, rx))
    }

    #[cfg(not(target_os = "windows"))]
    {
        let (tx, rx) = mpsc::channel();
        Ok((FocusWatcher {}, rx))
    }
}

/// 在现有定时器中消费焦点事件
pub fn poll_focus_events(rx: &mpsc::Receiver<FocusEvent>) -> Option<FocusEvent> {
    rx.try_recv().ok()
}

// ============================================================================
// Windows 实现
// ============================================================================

#[cfg(target_os = "windows")]
static mut FOCUS_EVENT_TX: Option<mpsc::Sender<FocusEvent>> = None;

#[cfg(target_os = "windows")]
fn get_foreground_process_name() -> Option<String> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 {
        return None;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buffer = [0u16; 512];
        let mut size = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size);
        CloseHandle(handle);
        if result != TRUE || size == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        let name = path.rsplit('\\').next().unwrap_or(&path).to_lowercase();
        Some(name)
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn win_event_proc(
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread_id: u32,
    _timestamp: u32,
) {
    if hwnd.is_null() {
        return;
    }

    if let Some(name) = get_foreground_process_name() {
        // 过滤 Clippi 自身窗口的焦点事件
        if name.contains("clippi") {
            return;
        }

        if let Some(ref tx) = FOCUS_EVENT_TX {
            let _ = tx.send(FocusEvent::ForegroundChanged(Some(name)));
        }
    }
}