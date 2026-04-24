use std::collections::HashSet;

#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::HWND;
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

pub fn get_focused_process_name() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        let hwnd: HWND = unsafe { GetForegroundWindow() };
        if hwnd.is_null() {
            return None;
        }

        let mut pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == 0 {
            return None;
        }

        get_process_name_by_pid(pid)
    }

    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[cfg(target_os = "windows")]
fn get_process_name_by_pid(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, TRUE};

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
        let name = path.rsplit('\\').next().unwrap_or(&path);
        Some(name.to_lowercase())
    }
}

pub fn is_blacklisted(name: &str, blacklist: &HashSet<String>) -> bool {
    let lower = name.to_lowercase();
    blacklist.iter().any(|item| lower.contains(&item.to_lowercase()))
}
